# ADR 06 — Diagnostic Collection and Stream Discipline

- **Status:** Accepted
- **Date:** 2026-08-03
- **Issue:** [MVP-04 — Diagnostic collection and stdout/stderr discipline](../todos.md#foundation)
  (`feat/diagnostics`)
- **Affects:** `cli/src/diagnostics.rs` (new), `cli/src/cli.rs`, `cli/src/sln.rs`,
  `cli/src/main.rs`, [engineering_spec.md](../engineering_spec.md) ("CLI Surface", "CLI Binary —
  Internal Modules"), [ADR 01](01-address-dependencies.md), [ADR 03](03-cli-surface-and-json-contract.md),
  [ADR 05](05-sln-configuration-mapping.md), MVP-07, MVP-13, MVP-21

## Problem

Three earlier issues each pushed a piece of this one forward and left it standing on its own.

MVP-01 deliberately added **no logging facade**, on the grounds that the diagnostic requirement that
actually matters — warn about unsupported MSBuild constructs — is structured data the plugin must
surface, not a log line, because the plugin is forbidden from scraping human-readable text. MVP-03
then settled the *shape* of that data: `Diagnostic`, `Severity`, the envelope's `warnings` array, and
`Envelope::with_warnings` as a builder deliberately separate from `ok`/`err`. And MVP-06 produced the
first actual diagnostic — the `.sln` format-version warning — which had nowhere to go, so it was
parked in a `Solution::warnings` field that ADR 05 explicitly flagged as temporary.

So the payload existed, a producer existed, and nothing connected them. `cli::run` returned
`Result<Outcome>` with no channel for warnings at all.

The second half of the issue is the `-v/--verbose` flag, defined by MVP-03 but inert, and the
invariant it endangers: under `--json` the plugin decodes stdout in its entirety, so stdout must
carry the envelope and nothing else. That invariant was documented and unasserted — and it was
already broken. `sln.rs`'s `split_row` carried a leftover `println!("Splitting line ...")` writing
debug output straight to stdout. It shipped because no CLI path reaches `sln::parse_sln` yet, so
every existing test looked past it.

## Decision

A new `cli/src/diagnostics.rs` owns `Diagnostic`, `Severity`, and a new `Reporter` that is threaded
through evaluation as `&mut`. `Reporter` holds both the accumulated warnings and the progress
channel: an `Option<Box<dyn Write>>` that is `Some(stderr)` under `-v` and `None` otherwise.
`Solution::warnings` is deleted; `sln::parse_sln` takes a reporter and calls `warn_at`.

`cli::run(&Cli, &mut Reporter)` reports the subcommand and solution as its first progress line, and
`main.rs` attaches `reporter.into_warnings()` to **both** the success and failure envelopes.

Warnings are written to stderr in *both* output modes, and additionally ride in the envelope under
`--json`. On stderr, a diagnostic's path is shown relative to the project root at default verbosity
and absolute under `-v`; the envelope always carries the absolute path.

The stray `println!` is gone, and an integration test now asserts that stdout under `--json` is
exactly one line containing exactly one JSON object.

## Discussion

**An accumulator threaded down, not warnings floated up.** The alternative was to keep doing what
MVP-06 did — return warnings alongside each result and merge them at the top — and for the code that
exists today it is plainly simpler: there is one producer, in one place, and `Solution::warnings`
worked. Four things decided it the other way.

The sharpest is that `?` destroys float-up warnings on exactly the path where they matter most.
`Envelope::with_warnings` exists *because* a failed run still has to report what it found before
failing; with float-up, `let (value, w) = inner()?;` discards `w` whenever `inner` errors, and
preserving it means `Result<(T, Vec<Diagnostic>), (Error, Vec<Diagnostic>)>` or a bespoke carrier —
at which point `?` stops working at all. An accumulator the caller owns is simply unaffected by
propagation.

Second, progress is inherently push-down. The point of `-v` is feedback *while* a long parse is in
flight; returning progress messages to be printed after the work finishes is strictly worse than not
having them. So *something* gets threaded regardless, and the real choice was between threading one
object and threading two.

Third, the evaluator's warning sites are not shaped like `sln.rs`'s. MVP-13's producers are
`expand_macro`, `eval_condition`, and `resolve_import`, which return `String`, `bool`, and `PathBuf`
and get called from inside iterator chains and recursive descent — the engineering spec notes that
import resolution interleaves with property expansion, so nodes get revisited. Float-up turns each of
those into a tuple return with a merge at every call site.

Fourth, order. The issue asks that diagnostics accumulate in order; an append-only collector makes
that true by construction, whereas merged sub-vectors are ordered only if every merge mirrors the
visit order, which is easy to get subtly wrong once nodes are revisited.

The cost accepted is one `&mut` parameter through the evaluation pipeline, and a constraint worth
writing down: `Reporter` must stay a standalone value rather than becoming a field of whatever state
the evaluator carries, or `&mut reporter` will collide with borrows of that state. Handlers already
take it while still returning `Error::Unimplemented`, so MVP-21 and MVP-22 inherit the parameter
instead of reopening every signature.

**The progress channel is an injectable sink, not a `verbose: bool`.** A bool plus `eprintln!` is
less machinery and needs no trait object. It was rejected because it leaves the issue's own test
requirement — that default verbosity suppresses progress — with nothing to assert against except the
integration test's "stderr happens to be empty." Making the sink `Option<Box<dyn Write>>` turns
suppression into a structural property: quiet means there is no sink, so no call site can bypass the
check, and a unit test can read back exactly what a loud reporter wrote. It also costs nothing to the
one future case that is easy to imagine — progress to a file — which a bool would have to grow a
second mechanism for. `Reporter` is consequently not `Debug` and not `Send`; nothing needs either.

**`Diagnostic` moved out of `cli.rs`.** It was defined there because MVP-03 owned the envelope, which
left `sln.rs` doing `use crate::cli::Diagnostic` — a parser reaching into the CLI layer. By MVP-13
every module in the evaluation pipeline produces diagnostics, and none of them should have to. The
module split follows `error.rs`: `diagnostics.rs` owns the type and how it reads to a human, `cli.rs`
owns how it is serialized as part of the envelope, and `main.rs` owns which stream it lands on.

**Warnings go to stderr in both modes, duplicating the envelope under `--json`.** Suppressing the
stderr copy when `--json` is passed would avoid printing the same thing twice on the plugin's own
path. It was rejected because MVP-13 asks for both explicitly, stderr is never parsed by anything, and
a captured `--json` run should still be legible without decoding JSON by hand. The other candidate —
warnings only under `-v` — was worse: the format-version warning exists precisely to tell someone that
the file they passed may not be a solution at all, and hiding that behind a flag defeats it. Warnings
are also written *before* the result or the error message, so an error stays the last thing on stderr.

**`render_human` lives in the library, not `main.rs`.** `main.rs` owns stream choice, and rendering a
warning line looks like its job. But `main.rs` is a binary crate: integration tests cannot reach into
it, and with `list-configs` still unimplemented there is no runtime path that would exercise the
relative-path logic either, so putting it there would have meant shipping untested path handling.
`Diagnostic::render_human(w, root, verbose)` is a library method with unit tests; `main.rs` decides
only that it goes to stderr. It deliberately does *not* implement the existing `Render` trait, which
is for stdout result types and passes no root or verbosity context.

**Paths are shortened at default verbosity.** Diagnostics carry absolute paths because the envelope
is a contract with the plugin and clangd consumers expect them. A terminal is a different audience:
CLAUDE.md §5 says not to put absolute user paths in anything shareable, and stderr text is exactly
what gets pasted into a bug report. So the path is shown relative to the root when it lives
underneath it, and `-v` is the explicit opt-in to the full path — which is what you want when the
shortened form is the thing confusing you. `--root` may be relative, so it is absolutized before
`strip_prefix`, which is purely lexical and would otherwise never match.

**The end-to-end test is present but ignored.** No subcommand reaches `sln::parse_sln` until MVP-21,
so "a real warning appears in the envelope" cannot be asserted through the binary yet. Partially
wiring `list-configs` to parse the solution and then still return `Unimplemented` was considered — it
would have made this demonstrable today, and would have caught the stray `println!` — but it lands
MVP-21's work in a half-state. The plumbing either side of the gap is unit-tested instead, and the
integration test is left `#[ignore]`d naming MVP-21, per CLAUDE.md §8's rule that gaps stay visible.

**stdout discipline is now asserted, not just documented.** The stray `println!` is the whole
argument: a debug print deep in a parser is invisible to every test that only checks what it expected
to find. `json_stdout_is_always_exactly_one_object` checks the shape of the whole stream — one line,
one object — across every `--json` invocation, so the next one fails a test instead of shipping.

## Consequences

- **MVP-07 through MVP-13 thread `&mut Reporter`** through evaluation, and MVP-13's producers call
  `Reporter::warn_at` rather than inventing their own channel. MVP-13 shrinks accordingly: the
  accumulation and reporting mechanism is done, and what remains is deciding which constructs warn.
- **MVP-21 unblocks `warnings_ride_in_the_envelope`** in `cli/tests/cli_surface.rs` and should extend
  `json_stdout_is_always_exactly_one_object`'s case list to the paths that actually parse a solution.
- **`Solution::warnings` is gone**, closing the temporary field ADR 05 introduced. `sln::parse_sln`
  and `parse_sln_text` both take a reporter.
- **`-v` is no longer inert**, so the engineering spec's CLI Surface table no longer says it is.
- **The `Reporter` must not become a field of evaluator state**, or `&mut` borrows will collide.
- **A stray stdout write is now a test failure**, which is what this issue was for.
- **New open question:** whether `Severity` needs a second level. One variant covers everything
  today — anything fatal is an `Error` — but MVP-13 may want to distinguish "this construct was
  skipped, your flags may be wrong" from "this is merely unusual", which are different things for a
  user to act on.
- **New open question:** whether the plugin should render warnings itself from the envelope, or
  whether MVP-28 folding stderr into its error output is enough. V2-04 owns error surfacing; nothing
  currently says which channel warnings reach the user through.
