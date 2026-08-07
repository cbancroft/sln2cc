# ADR 14 — Bounding XML Nesting Depth Before the Parse

- **Status:** Accepted
- **Date:** 2026-08-07
- **Issue:** [MVP-35 — Bound XML nesting depth before parsing](../todos.md#msbuild-evaluation)
  (`fix/xml-depth-guard`)
- **Affects:** `cli/src/msbuild/project.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 01](01-address-dependencies.md), [ADR 07](07-vcxproj-reader-ir.md),
  [ADR 08](08-eager-macro-expansion.md), [ADR 09](09-condition-evaluation-subset.md)

## Problem

`roxmltree::Document::parse` recurses once per nesting level. A debug build on a 2 MiB thread stack
survives about 100 levels and dies at 200 — from roughly 1.5 KB of XML. The process **aborts**:
there is no `Err` to propagate, no exit code, no JSON envelope, and nothing for the plugin to
surface. CLAUDE.md §5 requires that malformed and hostile project input fail gracefully rather than
hang or panic, and this was the one place in the crate where it did not.

ADR 07 found it while writing MVP-07's hostile-input test, wrote the test, and `#[ignore]`d it,
because running it takes the whole test binary down. That left the gap visible and unclosed.

The real margin is smaller than the measurement suggests. `imports.rs` recurses up to
`MAX_IMPORT_DEPTH = 64` frames and calls `parse_project` from the bottom of that walk, so the stack
`roxmltree` gets in practice is whatever the import walk has not already spent.

## Decision

`msbuild/project.rs` gains `MAX_XML_DEPTH = 32` and a private `check_nesting_depth(text, path)`,
called as the first statement of `parse_project_text`, before `Document::parse` sees the text.

It is a **byte scan, not a parse**. It walks the source once, counting element nesting: a start tag
opens a level, a close tag closes one, a self-closing tag does neither, and comments, CDATA
sections, processing instructions and declarations are skipped whole. Exceeding the cap is
`Error::RecursionLimit`, whose `context` names the file.

Depth is counted with `<Project>` as level 1, so a document may nest 31 levels below its root.

The scan produces no other error. Anything malformed — an unterminated tag, a stray `<`, a close
with no open — is stepped over and left to the parser, which reports it as `project-parse` exactly
as before.

## Discussion

**`roxmltree`'s own options are not the guard.** `allow_dtd` is already `false` by default, which
closes the entity-expansion surface and is why `a_dtd_is_rejected` passes; ADR 07 deliberately
leaves it there. `nodes_limit` looks like the answer and is not: it caps the *number* of nodes, and
cannot distinguish 5,000 nested elements from a large but entirely ordinary project with 5,000
source files. Any value low enough to stop the first refuses the second. The property that needs
bounding is depth, and the parser exposes no way to bound it.

**So it has to happen before the parse, which means it cannot be a parse.** There is no "after" —
the failure is an abort inside `Document::parse`, not a return value. Two alternatives to a byte
scan were weighed:

- *A second, lightweight XML parser* used only to measure depth. Rejected: it is the same work
  twice, and a hand-rolled parser that has to agree with `roxmltree` about what an element is has
  more ways to disagree than the scan does.
- *A different XML crate* with a depth option, or a streaming pull parser. Rejected as relitigating
  [ADR 01](01-address-dependencies.md) for one function. `quick-xml` was already considered and
  turned down there, and swapping the crate would rewrite the whole reader to buy one check.

The scan costs one linear pass over bytes already in memory, against a parse that walks the same
bytes and allocates a tree. It is not a measurable cost.

**`<` needs no quote tracking; `>` does, and the asymmetry is in the XML spec.** `AttValue` is
`'"' ([^<&"] | Reference)* '"'` — a literal `<` cannot appear inside an attribute value, so every
`<` in a well-formed file is markup and finding one is a plain byte search with no state. A literal
`>` *is* permitted there. That only matters because the scan has to find where a start tag **ends**
in order to know whether it closed itself, so `end_of_tag` tracks which quote character it is
inside. A dozen lines.

The cheaper version stops at the first `>` and was rejected on the grounds that its error is silent
and cumulative: each attribute containing a `>` leaks one level that is never closed, so the
failure appears in a file that is nowhere near the cap and gives no hint why. `>` in an attribute
value is rare — Visual Studio writes `&gt;` — but "rare" is the wrong basis for a check whose whole
purpose is to be trustworthy on input nobody vetted.

**Self-closing detection is load-bearing, not an optimisation.** Without it, depth grows with the
number of `<ClCompile Include="…" />` items in an `ItemGroup`, so a 200-source project reads as 200
levels deep. The guard would then refuse every real solution long before it refused a hostile one,
which is a worse outcome than not having it: a false rejection is indistinguishable, to the user,
from the tool being broken. Two of the seven tests exist for this case alone.

**A guard, not a validator.** It would be easy to have the scan report unbalanced tags, since it is
already counting them — and wrong. `Error::ProjectParse` carries `roxmltree`'s own message, with
its position and its description, and a second, worse diagnosis of the same file competing with it
helps nobody. So the scan's only failure mode is `RecursionLimit`, and everything else it steps
over. The two existing tests `malformed_xml_is_an_error_not_a_panic` and `a_dtd_is_rejected` become
the negative cases that keep it that way.

The same reasoning covers what the scan approximates. A `<!DOCTYPE>` with an internal subset, or a
name-start byte outside the ASCII range, may be counted differently than a real parser would count
it — but in every such case the file is one `roxmltree` rejects anyway, and the only question is
which of the two says so. Over-counting there costs a rejection on a file that was going to be
rejected.

**`MAX_XML_DEPTH = 32`.** Real project files reach four:
`Project > ItemDefinitionGroup > ClCompile > PreprocessorDefinitions`. Eight times that is generous
and deliberately no more generous. 64 was considered, to match `MAX_IMPORT_DEPTH`, and rejected
because the two caps compose rather than compete: the import walk may be 64 frames deep at the
moment a file is parsed, so what is left for `roxmltree` is well short of the ~100 levels a fresh
stack survived. 16 was considered as the tightest defensible value and rejected as buying nothing
over 32 while making an unusual-but-honest property sheet more likely to be refused. The number
matches `MAX_CONDITION_DEPTH`, which is the same kind of judgement about the same kind of risk.

The cost is that this is a guess about stack budget, not a measurement, and it is a guess made on
one platform in one build profile. A release build has deeper headroom and this cap will never be
near it.

**`Error::RecursionLimit` is reused.** ADR 08 already generalised the variant beyond literal depth
— it carries a byte count for the expansion cap — and had the `context` string say what the number
means. Here the number genuinely *is* a depth, so the fit is exact, and no new string enters the
plugin contract. `recursion-limit` already spans macro expansion, condition nesting and import
depth; XML nesting is the fourth thing it covers and the table row in the engineering spec says so.

**The message carries no line number.** `RecursionLimit { limit, context }` has no line field and
none of its three existing producers name one, and the scan would have to count newlines to
provide it. Rejected because a file that trips this is pathological by construction, and in the one
case where it is not — a real file that legitimately nests deeper than anyone expected — the
remediation is raising the constant, which the line number does not help with. If that case ever
turns up, the line is easy to add.

**It lives in `project.rs`, not in a new `msbuild/xml.rs`.** A separate module is arguably the
cleaner boundary: byte-level lexing is a different concern from lowering a tree, and `project.rs`
was already the longest module in the crate. It was rejected because the function has exactly one
caller, needs `pub(crate)` visibility it would not otherwise want, and its tests belong beside the
hostile-input tests that are already in `project.rs` — `a_dtd_is_rejected` and
`malformed_xml_is_an_error_not_a_panic` are negative cases for the guard, and splitting them from
it would leave neither half readable on its own.

## Consequences

- **`a_deeply_nested_document_is_rejected_rather_than_crashing` runs.** MVP-07's `#[ignore]` is
  removed and the suite has no ignored tests in the library target for the first time.
- **`ErrorKind::RecursionLimit` gains a fourth producer.** No wire string, exit code or plugin
  contract changes.
- **Every project file pays one extra linear scan.** Bounded by file size, well below the parse it
  precedes, and it exits early on the file it refuses.
- **The cap is a guess about stack budget**, made on a debug build on Windows. If a real project is
  ever refused, raising the constant is the fix and the test named
  `a_document_at_the_nesting_limit_still_parses` moves with it.
- **This closes the hazard ADR 07 named, not the whole class it belongs to.** File *size* remains
  unbounded: `parse_project` calls `read_to_string` and allocates whatever is on disk. Depth was the
  documented gap and is the one closed here; a cap on bytes read is a separate decision, belongs with
  the read rather than with the parser, and is deliberately not taken on now.
