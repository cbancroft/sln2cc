# ADR 13 — Diagnostic Severity and Warning Wording

- **Status:** Accepted
- **Date:** 2026-08-07
- **Issue:** [MVP-13 — Warn on unsupported MSBuild constructs](../todos.md#msbuild-evaluation)
  (`feat/unsupported-warnings`)
- **Affects:** `cli/src/diagnostics.rs`, `cli/src/sln.rs`, `cli/src/msbuild/eval.rs`,
  `cli/src/msbuild/imports.rs`, `cli/src/msbuild/items.rs`, `cli/src/msbuild/project.rs`,
  `cli/src/msbuild/flags.rs`, [engineering_spec.md](../engineering_spec.md) ("Diagnostics", "JSON
  envelope"), [ADR 06](06-diagnostic-collection.md), [ADR 08](08-eager-macro-expansion.md),
  [ADR 09](09-condition-evaluation-subset.md), [ADR 12](12-clcompile-flag-extraction.md), MVP-21,
  MVP-22, MVP-38

## Problem

CLAUDE.md §3 says a subtly wrong compilation database is worse than a loud failure, because clangd
will produce confident nonsense from it. MVP-13 is the issue that has to make that true.

Almost none of the machinery was left to build. MVP-04 built the channel — `Reporter`, the
envelope's `warnings` array, stderr rendering in both output modes, root-relative paths — and
MVP-07 through MVP-12 each threaded `&mut Reporter` and left a warning wherever they knew one
belonged, sixteen sites in six files. Six separate todo outcomes say some version of "MVP-13 owns
the wording."

What was actually left was the policy, and it had three problems.

**The channel was on course to drown itself.** Every finding was a `Severity::Warning`, because
that was the only variant. MVP-15 will supply `$(VCTargetsPath)` and start importing the Visual
Studio installation's own `.props`/`.targets`, at which point `` `$(X)` is not defined and expanded
to nothing`` fires on the order of hundreds of times per project, sitting in the same undifferentiated
list as the one finding that means a source file lost its include directory. The codebase already
reasons about this failure mode: ADR 12 declined to record an unset `%(Name)` on the grounds that
warning on something idiomatic "would train users to ignore the channel." A channel that is ignored
is worse than no channel, because it looks like diligence.

**The two most common messages existed in three copies.** `imports.rs::Walk::report`,
`items.rs::report`, and the inline loops inside `items.rs::holds` each formatted the same two
sentences from the `undefined`/`unsupported` lists that `Expansion` and `Evaluation` both carry.
Wording that is explicitly a decision cannot live in three places and stay one decision.

**Three things pointed at this issue and were still open.** ADR 06 left `Severity`'s second level
as an open question naming MVP-13. `flags.rs:221` marked the whitespace split of `AdditionalOptions`
as this issue's call. `eval.rs`'s `a_bare_percent_or_at_is_left_alone` said the same about `%XX`
escape decoding. And the engineering spec carried an open question about property values with
element children — "the one place the reader drops something without saying so", which is precisely
this issue's subject.

## Decision

**`Severity` gains a second level, `Note`, split by one question:** *could msvclsp's failure to
model this have changed the compiler flags?* Yes is a `Warning`; no is a `Note`. Wire strings are
`"warning"` and `"note"`, written literally. `Reporter::warn_at` keeps its meaning and a sibling
`Reporter::note_at` is added, so only the sites that change severity change at all. `Severity::label`
gives stderr the same word the envelope carries, with a test pinning the two together.

Four of the sixteen existing sites become notes: an unexpected (but present) `.sln` format version,
an item declaring none of `Include`/`Remove`/`Update`, a file already imported, and a wildcard in a
`Remove` or `Exclude`. Everything else stays a warning. The full catalogue is in the engineering
spec's Diagnostics section.

**Both levels are reported through the same channel.** Notes are not suppressed by default, not
hidden behind `-v`, and not held back from the envelope.

**The two repeated messages get one definition**, `eval::report`, beside the `Expansion` and
`Evaluation` types that produce the findings. `Expansion::report` and `Evaluation::report` are
consuming wrappers returning `value` and `holds`. Site-specific wordings stay at their sites.

**`Reporter` suppresses exact duplicates** — a diagnostic identical in severity, message, file and
line to one already recorded is dropped, keeping the first.

**Two leftovers are settled.** A `"` in `AdditionalOptions` now warns that whitespace splitting will
break it apart, with the real fix deferred to MVP-38. A property value carrying element children now
warns rather than silently keeping only its text.

**One is settled as "no change".** `%XX` escapes stay undecoded and unwarned.

## Discussion

**Why a second level rather than better wording.** The alternative was to keep one level and carry
the distinction in the sentence — "note that…" versus "…may be wrong". It is less machinery, and
nothing consumes `severity` yet, so it could not break anything today. It was rejected on two
grounds. The first is that the distinction has to survive *sorting and filtering*, which is what a
consumer will actually do with hundreds of findings, and prose does not survive either. The second
is timing: adding a variant is free right now and stops being free the moment the plugin decodes the
field (MVP-28), because a Lua consumer that branches exhaustively on `"warning"` breaks on a value
it has never seen. Paying a cost now that is strictly lower than the same cost later, for a
distinction we already know we need, is the easy version of this trade.

**Why one question and not a taxonomy.** The tempting design is a richer classification —
unsupported-construct, unresolved-reference, dropped-content, unusual-input — which would let a
consumer filter by kind rather than by seriousness. It was rejected because nothing has asked for
it, and because the question a user actually has in front of a wall of output is not "what kind of
finding is this" but "do I have to care." One question with a yes/no answer is answerable
consistently at every call site by whoever adds the next one; a four-way taxonomy is a thing to
argue about in every review. A kind field can be added later without moving the severity, if a
consumer ever wants one.

**Why notes are not suppressed by default.** Making `Note` an opt-in under `-v` is the obvious way
to shrink default output, and it was rejected for the reason ADR 06 already rejected it for
warnings generally: the format-version finding exists precisely to tell someone the file they
passed may not be a solution, and a finding that only appears when you already suspect something is
a finding that does not do its job. Suppression is also the *consumer's* decision, not the
producer's — the plugin has a UI and knows what the user is doing, and the CLI has neither. What
the split buys is that the consumer now *can* decide; it does not require the CLI to decide first.

**The classification's two arguable rows.** A repeat import is a note because MSBuild's own MSB4011
rule is to import a file once per project — msvclsp did the same thing MSBuild would, so nothing was
lost, and a diamond-shaped sheet graph is common enough that warning would be noise on correct
input. A *duplicate source declaration* goes the other way and stays a warning, even though the file
still ends up in the database with flags: the dropped declaration may carry per-file metadata, so
settings that were asked for did not arrive. The line is "did anything get lost", not "does output
still exist".

**The wildcard rows are a bug fix wearing a severity change.** `specs` warned on any unexpanded
wildcard, including one in a `Remove` or an `Exclude`. In the usual pairing that is a false
positive: an `Include` wildcard is not expanded either, so the list an `Exclude` subtracts from
never received the files it would have removed, and nothing is lost on input msvclsp handled
correctly. It needed the severity split to fix properly, because the right answer is not silence —
the construct really was not modelled, and MVP-37 may one day change what it means.

> **Resolved by [ADR 29](29-wildcard-item-expansion.md).** MVP-37 expanded wildcards for real,
> which made every case below moot at once: `Wildcard`, `include_item`'s `exclusion` reading,
> and the `Remove` gap two paragraphs down all went away together, since a real match makes the
> "what does an unexpanded wildcard cost" question stop applying. Left in place rather than
> edited, per the house rule that a superseded ADR keeps its original argument.

But "an `Exclude` wildcard is harmless" is only true when its `Include` put nothing within reach.
`Include="main.cpp;foo.generated.cpp" Exclude="*.generated.cpp"` has real files to subtract from,
and not expanding the `Exclude` leaves `foo.generated.cpp` in the database — clangd is then handed a
translation unit the build never compiles. So the reading is decided in `include_item`, which is the
only place that knows both halves: `Harmless` when the `Include` produced no paths, and a third
variant, `Keeps`, when it did. `Keeps` is a warning.

Three variants rather than two because the two costly cases **point in opposite directions and
cannot share a sentence**: an unexpanded `Include` leaves files *out* of the database, an unexpanded
`Exclude` leaves them *in*. Reusing `Loses`' wording for an `Exclude` would have told the user
exactly the wrong thing about which files to go looking for. Deciding on emptiness rather than on
whether the `Include` was *written* as a wildcard is deliberate and slightly broader: a literal spec
that expands to nothing through an undefined property leaves the `Exclude` with nothing in reach
just the same, and there is a test for it. An `Update` wildcard stays `Loses`, since what it loses
is per-file settings rather than files.

The `Remove` arm in `settle` still passes `Harmless` unconditionally and has the same latent hole —
a `Remove` wildcard in a group following literal `Include`s fails to remove them the same way. It is
left alone because `Remove` subtracts from every source collected so far rather than from one
sibling `Include`, so the "nothing in reach" test is a different question there and answering it
properly means first deciding whether that whole-project subtraction is even the right MSBuild
semantics. Written down rather than fixed quietly.

**Why only two of the messages centralise.** The instinct is to pull every wording into one module
so the catalogue is readable in one place — which is genuinely what a reviewer wants. It was
rejected for the fourteen site-specific messages because each one is only defensible next to the
comment explaining the construct it describes: `items.rs` explains why a duplicate declaration
cannot be kept, `flags.rs` explains why an unrecognized *value* warns while an unrecognized *setting
name* does not. Moving the sentence away from that reasoning makes both harder to change correctly.
The two that do centralise are different in kind: they are produced mechanically from
`Expansion`/`Evaluation` at three unrelated call sites that know nothing about each other, and the
duplication was already real rather than hypothetical. The readable-in-one-place requirement is met
by the engineering spec's catalogue instead, which is the right medium for it.

**`eval::report` lives in `eval.rs` despite ADR 08 keeping `expand` reporter-free.** ADR 08 was
deliberate that `expand` takes no `Reporter` and warns about nothing, so that MVP-13 could own every
word. That still holds: `expand` and `condition_holds` are unchanged and still return findings
rather than emitting them. What is new is a separate function that turns findings into sentences,
which belongs beside the types whose fields it reads. The alternative homes were `diagnostics.rs`,
which would put MSBuild vocabulary in a module that deliberately knows nothing about MSBuild, and
`msbuild.rs`, where nothing else lives that either caller uses.

**Duplicate suppression is exact-match only.** Anything cleverer — collapsing the same message
across different lines into "and 40 more" — was rejected as a display concern that belongs to
whoever renders, not to the accumulator, and as something that would make `warnings()` lossy in a
way a test cannot easily pin. Exact match is safe because two findings identical in all four fields
describe one construct reported twice; there is no reading under which the second informs anybody.
The cost is a `HashSet<Diagnostic>` of clones alongside the vector, which is trivial next to the XML
being parsed, and a `Hash` derive on two types. This matters most for MVP-22, which will evaluate
every project in a solution against one reporter, and where a `Directory.Build.props` shared by
thirty projects would otherwise repeat each of its findings thirty times.

**`AdditionalOptions` warns rather than being fixed.** A shell-style splitter respecting quotes is
maybe twenty lines and would be *correct*, which is a real argument for doing it here. It was
deferred because getting it right means picking whose quoting rules to implement — MSBuild's,
`cl`'s response-file rules, or the Windows command-line parser's, which disagree about backslashes
before quotes — and that is a decision with its own testing surface, not a detail of a warnings
issue. The interim state is honest: the corruption still happens, and now it announces itself.
MVP-38 holds the work, and the spec's existing open question points at it.

**`%XX` escapes stay undecoded and unwarned.** Decoding them is arguably correct MSBuild behaviour,
and was rejected here as a change to evaluation semantics rather than to diagnostics — it belongs to
whoever owns `eval.rs` next, with tests for the escape table. Warning about them was also considered
and rejected: a precise check for `%` followed by two hex digits would still fire on ordinary paths
and URLs that contain one, and the failure it would be warning about is visible in the generated
database anyway, where a stray `%3B` in a define is self-explaining. Silence here is a considered
position rather than an oversight, which is why it is written down.

**Property values with element children now warn.** This closes a spec open question rather than
one of MVP-13's own. `text_of` keeps a value's text and discards element children, which is the
right recovery — the construct is not valid MSBuild and there is nothing else to do with them — but
it was the only place the reader lost content in silence. The exposure is small, which is the
argument for leaving it; the counter-argument that won is that "we drop this silently, but only
rarely" is exactly the shape of the bug this issue exists to prevent, and the check is one
`any(is_element)` per setting. It costs a `path` and a `reporter` parameter on `settings` and its
two callers. It does not false-positive on the two-level shapes real files use, because a
`ProjectConfiguration`'s children and an `ItemDefinitionGroup`'s metadata are reached *through*
`settings` rather than seen by it.

## Consequences

- **`Severity` has two variants and the envelope can carry either.** `schema_version` is not bumped:
  the shape is unchanged and a new enum *value* is additive. **MVP-28 must not branch exhaustively**
  on the severity string, and should treat an unknown value as a warning.
- **ADR 06's open question is resolved**, and removed from the engineering spec's Open Questions.
- **The spec's "property value containing element children" open question is resolved** and removed.
- **MVP-38 is new** — shell-style `AdditionalOptions` splitting — and the spec's open question on
  argument splitting now names it.
- **`Reporter::record` (formerly `warn`) is no longer append-only.** It still preserves order and
  still keeps everything distinct; a producer that deliberately wants to count occurrences cannot
  use it to do so. The rename is because "warn" no longer describes a function that also takes
  notes.
- **`Reporter` must stay cheap to clone into.** `Diagnostic` now derives `Hash`, so any field added
  to it has to be hashable.
- **New call sites pick a severity by answering one question**, and the catalogue in the engineering
  spec is expected to grow a row when they do.
- **MVP-21 unblocks `warnings_ride_in_the_envelope`**, which now also owes an assertion that the
  severity reaches the wire — a level that never leaves the library is indistinguishable from not
  having split it.
- **MVP-15 is the real test of this.** The severity split is a prediction about what importing the
  Visual Studio installation's own sheets will do to the volume of findings. If the warning half is
  still a wall after that lands, the next move is filtering by origin — findings from files under the
  VS installation are not the user's to act on — not a third severity.
