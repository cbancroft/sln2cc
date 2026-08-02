# ADR 03 — The CLI Surface and the JSON Envelope

- **Status:** Accepted
- **Date:** 2026-08-02
- **Issue:** [MVP-03 — CLI skeleton with subcommand dispatch](../todos.md#mvp) (`feat/cli-skeleton`)
- **Affects:** `cli/src/cli.rs`, `cli/src/main.rs`, `cli/src/error.rs`, `cli/src/lib.rs`,
  `cli/tests/cli_surface.rs` (new), [engineering_spec.md](../engineering_spec.md) ("CLI Surface"),
  [CLAUDE.md](../../CLAUDE.md) §3, MVP-04, MVP-21, MVP-22, MVP-28, V1-01, V2-04

## Problem

`main.rs` printed "not yet implemented" and exited 1. Nothing parsed arguments, and the payload
shape the Neovim plugin decodes did not exist — `cli.rs` held two bare result structs with no
`serde` derives.

Two open questions converged here. `engineering_spec.md` carried "CLI subcommand surface and JSON
output contract with the Neovim plugin" from `project_spec.md`, unanswered. And
[ADR 02](02-structured-error-type.md) deliberately stopped short of serialization: `ErrorKind`
derives no `serde` traits, on the grounds that the payload's shape belongs to whichever type owns
the output. That is this issue.

The question is not just "what are the flags". It is what the plugin decodes on success, what it
decodes on failure, how it tells the two apart, and where warnings live once MVP-04 produces them.
Those answers are load-bearing from MVP-21 onward and expensive to change afterward.

## Decision

Two subcommands, `list-configs` and `generate`, each taking the solution as a required positional
and sharing a `--root`. `generate` additionally takes `--configuration` and `--platform` as
separate flags, plus `--force`. `--json` and `--verbose` are global.

Human-readable text is the stdout default; `--json` opts into a machine-readable envelope. Under
`--json`, stdout carries exactly one JSON object and nothing else. stderr carries the
human-readable message in *both* modes. `clap`'s usage errors (exit 2) are a carve-out: `clap`
writes its own text to stderr, stdout stays empty, and no envelope is produced.

The envelope has a fully static shape — all five keys present on success and failure alike:

```json
{ "schema_version": 1, "has_error": false, "result": {}, "error": null, "warnings": [] }
```

`has_error` is the discriminator. `error` carries `kind` (`ErrorKind::as_str()`), `message`, and
`causes` (the `source()` chain, outermost-inward). `warnings` is declared now and filled by MVP-04.

A temporary `Error::Unimplemented` variant — wire string `"unimplemented"`, exit code `9` — is what
the handlers return until MVP-21 and MVP-22 replace them.

The surface and envelope are recorded in "CLI Surface" in
[engineering_spec.md](../engineering_spec.md), which closes that open question.

## Discussion

**`--json` opt-in rather than JSON always.** The stronger engineering argument was always-JSON: one
rendering path, no second renderer to keep in sync, and the "stdout is decodable" invariant true by
construction rather than conditionally. Pretty-printing would have kept it readable by hand at no
cost, since `vim.json.decode` ignores whitespace. It was rejected in favour of a genuine
human-readable default, because the tool is run by hand constantly while debugging the evaluator
and a JSON dump is a worse experience than a two-line table every single time. The cost is real and
accepted: two renderers per output type forever, and a failure mode where a plugin call site that
omits `--json` gets undecodable text and presents as clangd mysteriously not attaching. Inverting
the flag — JSON by default, `--human` to opt out — was considered and would have removed that
failure mode while keeping the readable output; it was rejected because the flag a human types
should be the unusual one, not the common one. The mitigation is `both_subcommands_accept_json`,
which MVP-28 extends to assert against the argv `cli.lua` actually builds.

**A static envelope, and why `has_error` is not redundant.** The obvious objection to `has_error`
is that `error == null` already says the same thing. It does not, on the consumer that matters:
`vim.json.decode` maps JSON `null` to `vim.NIL`, and `vim.NIL` is *truthy* in Lua. A plugin author
writing the natural `if result.error then` would take the error branch on every successful call.
The alternatives were comparing against `vim.NIL` at each call site, which is a footgun waiting for
the one place someone forgets, or omitting `error` entirely on success, which makes the shape vary
by outcome and forces the documentation to describe two structures. An explicit boolean costs one
field and makes the wrong reading impossible. The same reasoning keeps `result` present-and-null on
failure: one shape, documented once. No field carries `skip_serializing_if`, and that is a
deliberate constraint rather than an oversight.

**Where the error JSON goes.** Writing the structured error to stderr alongside the human message
was rejected: it puts machine and human text on one stream and makes the plugin separate them,
which is the scraping CLAUDE.md §3 forbids. Putting it on stdout keeps each stream single-purpose —
stdout is decodable in its entirety or empty, never partly. stderr still carries the human message
in JSON mode, because MVP-28 folds stderr's contents into the plugin's own structured error on a
non-zero exit, so discarding it would lose the most legible description of the failure.

**Separate `--configuration` and `--platform` over one `Debug|x64`.** The combined form matches how
solutions and MSBuild conditions write the pair, and `ConfigPlatform` already renders that way. It
was rejected because `|` is a pipe in every shell the tool will be run from, so every by-hand
invocation would need quoting, and a malformed value becomes a parse error this crate has to define
and report. Two flags map one-to-one onto the struct's two fields and cost nothing but verbosity.
The combined form remains available later as an additional spelling if it is ever wanted.

**`Unimplemented` rather than `todo!()`.** MVP-03's own description authorized `todo!()` in the
handlers. It conflicts with CLAUDE.md §5's prohibition on panics in library paths reachable from
the CLI, and the conflict is not theoretical: anyone running `msvclsp generate` by hand before
MVP-22 lands would get a panic and exit 101. Worse, the exit-code and envelope plumbing this issue
exists to build would have had no path exercising it end to end, so its tests could only have
asserted on `clap`'s behavior. A leaf variant makes the failure structured, gives the integration
tests something real to assert against, and costs one wire string documented as temporary. Reusing
an existing variant was rejected outright — the kind is what the plugin dispatches on, and
reporting a false one is worse than reporting a placeholder.

**Defining `--force` and `--verbose` before they do anything.** Both belong to later issues (MVP-22
and MVP-04). Leaving them out would mean the surface recorded here as "final" gains flags in the
next two issues, which makes the record worthless. Defining them inert costs two unused fields and
makes the contract genuinely settled.

**`discover` deliberately absent.** `cli.rs` already holds a `DiscoveredSolution` type and
`cli.lua` stubs a `discover` call, so defining the subcommand now was tempting. It is V1-01, and
the MVP plugin scans in Lua (MVP-27). Adding a subcommand later is purely additive and breaks no
existing invocation, whereas defining one now means guessing its output shape with nothing
consuming it. The type stays as a V1 placeholder.

**Exit code `9` for `unimplemented`.** It extends ADR 02's table by one class rather than reusing
`1`, so a by-hand invocation can tell "this isn't built yet" from a genuine I/O failure. It leaves
with MVP-22.

**`&mut dyn Write` in `Render` rather than `impl Write`.** Generic writers monomorphize a copy of
each renderer per writer type and make the trait dyn-incompatible, buying nothing for output that
is I/O-bound. Dynamic dispatch also keeps `Box<dyn Render>` available if the subcommand list grows.

## Consequences

- The open question "CLI subcommand surface and JSON output contract with the Neovim plugin" is
  closed and removed from `engineering_spec.md`.
- **CLAUDE.md §3 changes.** "Output consumed by the plugin is JSON on stdout" becomes conditional
  on `--json`. The underlying rule — the plugin never scrapes human-readable text — is unchanged.
- **MVP-04's invariant is reworded** from "nothing but JSON is ever written to stdout" to "nothing
  but the envelope under `--json`". Its `Diagnostic` type and the `warnings` field already exist;
  MVP-04 owns accumulating into them and wiring `--verbose`.
- **MVP-21 and MVP-22 narrow** to implementing `list_configs` and `generate` behind a settled
  signature, and each removes its `Unimplemented` call site. Whichever lands second removes the
  variant, the kind, and exit code `9`.
- **MVP-28 gains a fixed contract**: pass `--json`, decode stdout, branch on `has_error`, and fold
  stderr into the error on a non-zero exit. It should extend `both_subcommands_accept_json` to
  cover the argv it builds.
- **V2-04 gains a stable payload** with `kind` and `causes` to render.
- `ErrorKind::ALL` grew to fifteen, which broke three tests in `error.rs` by design — the literal
  wire-string list is asserted precisely so this cannot happen silently.
- **New open question:** whether the plugin should ever verify `schema_version`, or whether the
  field exists only for a human reading a captured payload. Nothing enforces it today.
