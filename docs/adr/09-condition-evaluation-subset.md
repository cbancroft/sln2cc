# ADR 09 — The `Condition` Subset, and What Happens Outside It

- **Status:** Accepted
- **Date:** 2026-08-04
- **Issue:** [MVP-09 — `Condition` expression evaluation](../todos.md#msbuild-evaluation) (`feat/msbuild-conditions`)
- **Affects:** `cli/src/msbuild/eval.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 02](02-structured-error-type.md), [ADR 06](06-diagnostic-collection.md),
  [ADR 07](07-vcxproj-reader-ir.md), [ADR 08](08-eager-macro-expansion.md),
  MVP-10, MVP-12, MVP-13

## Problem

ADR 07 carries every `Condition` forward as a raw string and ADR 08 can expand the `$(...)` inside
one, but nothing decides whether a `PropertyGroup` applies. That decision is what selecting a
configuration *means*: without it MVP-12 cannot tell the `Debug|x64` `ItemDefinitionGroup` from the
`Release|Win32` one, and MVP-10 cannot skip an import that evaluates false.

MSBuild's condition grammar is large — numeric and version comparison, a dozen condition functions,
property functions, item transforms, registry reads. Implementing all of it is not the MVP's
business. So the question is not really "how do we evaluate conditions"; it is **what do we do with
the ones we decline to evaluate**, given that a silently skipped `PropertyGroup` produces a
compilation database that looks right and is wrong.

## Decision

`msbuild/eval.rs` gains a tokenizer and a recursive-descent evaluator over this grammar:

```
condition := or
or        := and ( 'Or' and )*
and       := unary ( 'And' unary )*
unary     := '!' unary | primary
primary   := '(' condition ')' | 'Exists' '(' operand ')'
           | operand ( ('==' | '!=') operand )?
operand   := "'" text "'" | bare-token
```

`condition_holds(condition, props, file, line) -> Result<Evaluation>`, where
`Evaluation { holds, undefined, unsupported }` mirrors ADR 08's `Expansion`. `And`, `Or` and
`Exists` match case-insensitively, comparison is case-insensitive, and an empty condition holds.

**Unsupported syntax is an `Error::UnsupportedConstruct`** — MVP-09 is that variant's first
producer. **An unsupported `$(...)` inside an operand is recorded**, not fatal.

## Discussion

**Two kinds of "unsupported", and separating them is the whole design.** A construct splits by
whether an answer exists at all:

- `'1' < '2'` or `HasTrailingSlash('$(OutDir)')` is an expression this evaluator cannot reduce to a
  boolean. There is no truth value to return, so it errors.
- `'$([System.IO.Path]::GetTempPath())' == 'C:\temp\'` is an expression it *can* reduce: ADR 08
  left the construct verbatim, so there are two strings and the comparison runs. The answer may
  well be wrong, which is exactly what the recorded finding tells the user.

Collapsing the two was considered in both directions. Erroring on the second would make an
unevaluable *value* as fatal as an unevaluable *expression*, which they are not — and would take
down runs over a property nothing important reads. Recording the first would require inventing a
boolean, and both choices are bad: `false` silently drops a group that may carry every include path
in the project, `true` applies flags the file did not ask for. CLAUDE.md §3 says a subtly wrong
database is worse than a loud failure, and this is the case it was written about.

**The error is fatal here, and MVP-10 owns whether it stays that way.** The cost is real and known:
MVP-10 will import Microsoft's own `.props`/`.targets` from the Visual Studio installation, and
those use constructs well outside this subset — `$([MSBuild]::VersionGreaterThanOrEquals(...))`,
`HasTrailingSlash`, numeric comparison. Taken literally, this decision makes the tool fail on every
real solution.

Two ways out were weighed. A tri-state return (`Holds(bool)` / `Unknown(construct)`) makes the
caller confront the choice at the call site, but spreads condition policy across every caller and
leaves `UnsupportedConstruct` producerless again. Erroring keeps MVP-09 honest — it genuinely has
no answer — and because ADR 02 preserves `kind()` through propagation, MVP-10 can catch it and
decide to degrade based on something MVP-09 cannot see: whether the file came from the user's tree
or from the toolchain. Deciding that policy now, with no importer to test it against, would be
guessing. It is recorded as MVP-10's inheritance rather than pre-empted.

**Parse first, expand second.** The simpler implementation expands the whole condition string and
then parses the result. It was rejected on security grounds. Project files are untrusted input
(CLAUDE.md §5), and a property whose value is `literal' Or 'a'=='a` would, under expand-first, turn
`'$(Evil)'=='literal'` into `'literal' Or 'a'=='a'=='literal'` — a different expression, forcing a
group to apply. Quoting is no defence when the operand supplies its own quotes. Parsing first means
an operand's text can never become an operator; there is a test named for exactly this.

The cost is that the tokenizer must handle `$(...)` without expanding it: `$(Configuration)`
contains a `)`, so a bare token that ended at the first `)` would split one operand into three.
ADR 08's `closing_paren` is reused to skip a reference whole.

**No short-circuit.** MSBuild stops evaluating `And` once the left side is false. This evaluator
does not, so both sides always run. Nothing in the subset has side effects — `Exists()` only asks
the filesystem a question — so the boolean result is identical. What differs is that an unsupported
construct on the dead side still gets reported. Not understanding half a condition is worth knowing
about even when the other half settled the answer. The cost is a handful of `Exists()` calls MSBuild
would have skipped, and a condition that MSBuild would have accepted being refused here.

**Comparison is case-insensitive, and numeric comparison is absent.** MSBuild compares condition
strings without regard to case, so `'$(Configuration)'=='debug'` matches `Debug`; comparing exactly
would skip a group MSBuild applies. MSBuild *also* compares numerically when both operands parse as
numbers, which makes `'1.0' == '1.00'` true where it is false here. That is a real infidelity, left
in place because the todo's subset does not include it, C++ property sheets compare toolset and
configuration names rather than numbers, and float equality is its own trap. Recorded as an open
question rather than quietly implemented.

**`Exists()` touches the real filesystem, and the tests do too.** Relative paths resolve against the
directory of the file the condition is written in, as MSBuild resolves them — not against the
process working directory, which would make the answer depend on where the user ran the tool. That
is why `file` is a parameter rather than just an error site.

No filesystem abstraction was introduced. The `ToolchainProvider` precedent exists because real
Visual Studio paths only exist on Windows and differ per machine; `Cargo.toml` under
`CARGO_MANIFEST_DIR` exists on every machine that can run the suite, so a fake would add an
indirection to test nothing. `Exists()` also reads nothing and executes nothing, which keeps it
inside CLAUDE.md §5's "parse them, don't evaluate them as commands".

`..` traversal is resolved rather than refused. `Error::PathEscape` exists, but ADR 04 already
established that projects outside the solution tree are ordinary, and
`Exists('..\shared\common.props')` is a legitimate and common thing for a property sheet to ask.

**Bounded depth.** Recursive descent makes `((((...))))` a stack-overflow vector on untrusted input
— the same class of bug MVP-35 owns for XML nesting, and unlike that one this test can actually run
without taking the test binary with it. `MAX_CONDITION_DEPTH = 32` against real conditions that nest
two or three deep, raised as `Error::RecursionLimit`, which ADR 08 already generalised beyond depth.

**The error names the whole condition, not the offending token.** A token on its own (`<`) gives a
user nothing to search for. A condition is short enough to quote in full, and `truncate` caps it.

**No AST.** The parser evaluates as it goes. Building a tree would add a shape to keep in step with
the grammar, and nothing needs the tree once the boolean is known. If MVP-13 later wants to explain
*which sub-expression* was undecidable, that is when a tree earns its place.

## Consequences

- **MVP-10 inherits the fatal-error policy** and owns whether to catch `UnsupportedConstruct` and
  degrade to a warning for files that came from the Visual Studio installation rather than the
  user's tree. This is the decision most likely to be revisited, and it is expected to be.
- **MVP-12 evaluates a group's condition and its children's separately**, which is what ADR 07 kept
  them apart for.
- **MVP-13 consumes `Evaluation::undefined` and `Evaluation::unsupported`** alongside `Expansion`'s,
  and must decide whether an undefined property inside a condition is worth reporting at all —
  `'$(X)' != ''` is how a property sheet legitimately tests for absence, so this will be the noisiest
  source of findings by far.
- **`Error::UnsupportedConstruct` finally has a producer**, and its exit code 5 is now reachable.
- **New open question:** numeric comparison. `'1.0' == '1.00'` is true in MSBuild and false here.
- **New open question:** a `\` path separator inside `Exists()` is one ordinary filename character
  on a non-Windows build. The crate must build everywhere but only runs on Windows, so this only
  affects tests today — MVP-10 resolves import paths and will meet the same question for real.
