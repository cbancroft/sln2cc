# ADR 07 — The `.vcxproj` Reader and Its Intermediate Representation

- **Status:** Accepted
- **Date:** 2026-08-03
- **Issue:** [MVP-07 — Read `.vcxproj` structure](../todos.md#msbuild-evaluation) (`feat/vcxproj-reader`)
- **Affects:** `cli/src/msbuild/project.rs` (new), `cli/src/msbuild.rs`, `cli/src/error.rs`,
  [engineering_spec.md](../engineering_spec.md) ("CLI Binary — Internal Modules"),
  [ADR 01](01-address-dependencies.md), [ADR 02](02-structured-error-type.md),
  [ADR 06](06-diagnostic-collection.md), MVP-08, MVP-09, MVP-10, MVP-12, MVP-13, MVP-35

## Problem

Everything downstream of `msbuild::evaluate` — macro expansion, condition evaluation, import
resolution, flag extraction — needs the same input, and nothing in the crate could read XML yet.

The requirement that shapes the answer is that **evaluation order is the semantics**. Whether
`Debug|x64`'s `PropertyGroup` beats the one above it is decided by position in the file, and the
same is true of the `Import`s that pull in property sheets. Any representation that collects
assignments into a map keyed by property name has already destroyed the answer, and destroyed it
invisibly: the result still looks like a property bag.

ADR 01 chose `roxmltree` and flagged the consequence to design around — it borrows from the source
`&str`, so lifetimes propagate — and recorded the intended shape: parse one file at a time, lower
it into an owned IR, drop the `Document` before reading the next import.

## Decision

A new `cli/src/msbuild/project.rs` lowers one MSBuild project file into
`ProjectFile { path, elements: Vec<Element> }`, where `Element` is a top-level child of `<Project>`
and the `Vec` is in document order. It reads `.vcxproj`, `.props` and `.targets` alike — the format
and the root element are the same, and MVP-10 reads property sheets through this same function,
which is why the module is named for the root element rather than for `.vcxproj`.

No evaluation happens: no macro is expanded, no condition is evaluated, no import is followed, no
path is resolved. Every `Condition` is carried forward as a string.

- `PropertyGroup`, `ItemDefinitionGroup`, `ItemGroup`, `Import` and `ImportGroup` are modelled.
  Each carries its own `Condition` and its 1-based line.
- One `Setting { name, value, condition, line }` type covers both property assignments and item
  metadata.
- `Target`, `UsingTask` and `ProjectExtensions` are skipped silently. Anything else unmodelled
  becomes `Element::Unsupported { name, line }` **and** a warning on the threaded `Reporter`.
- Elements are matched on their local name, so the MSBuild namespace is optional.
- Property and metadata values are trimmed; conditions and other attributes are kept exactly as
  `roxmltree` decodes them.
- `ProjectFile::items_of_type` returns candidate items of a type across all groups, in document
  order, matched case-insensitively.

`error.rs` gains `Error::ProjectSchema { path, line, detail }` and a matching
`ErrorKind::ProjectSchema` (`"project-schema"`, exit code 4) for well-formed XML that is not a
project MSBuild could evaluate.

## Discussion

**A vector of elements, not a property bag.** The obvious shape is what the consumer eventually
wants — a `HashMap<String, String>` of properties, a `Vec<PathBuf>` of sources — and building it
during the read would save a pass. It was rejected because the read is the one place that still
knows the order, and the order is the entire difficulty of MSBuild evaluation. Conditions make this
sharper than plain last-write-wins: two `PropertyGroup`s may assign `ConfigurationType`, and which
applies depends on conditions this pass deliberately does not evaluate, so there is no correct
merge available here at all. Duplicates are kept for the same reason — inside one group as well as
across groups.

The cost is a second pass and a shape that is more verbose than what any single consumer wants.
That is the intended trade: the IR is faithful to the file, and each consumer projects out of it.

**One `Setting` type for properties and metadata.** MSBuild treats a property and an item's
metadata as genuinely different things, and two structs would record that. They were rejected
because the XML is identical — `<Name Condition="...">value</Name>` — and so is what every later
pass does with it: expand the value, check the condition. Two structurally identical types would
duplicate the shape, the parse, and every helper, to preserve a distinction that is already
recoverable from where the value sits in the tree. If a later pass needs the two to diverge, the
split is mechanical.

**Group conditions stay separate from their children's.** An `ImportGroup Condition="A"` containing
`Import Condition="B"` could be flattened into one `Import` with condition `(A) and (B)`, which
would let MVP-10 treat imports uniformly. Rejected: it puts an expression in the IR that appears in
no file, and the moment the reader synthesizes syntax it has started evaluating. It would also have
to know MSBuild's operator spelling and precedence — which is MVP-09's subject, not this one's — and
a diagnostic quoting a condition the user never wrote is worse than no diagnostic. `PropertyGroup`
gates its properties the same way and is modelled the same way.

**Namespaces are ignored.** Visual Studio always writes
`xmlns="http://schemas.microsoft.com/developer/msbuild/2003"`; hand-written property sheets
routinely omit it, and MSBuild reads both. Matching on the local name makes the two identical here,
and the alternative — requiring the namespace — would reject files that build in Visual Studio.
Element names are matched case-sensitively, so `<propertygroup>` lands in the `Unsupported` arm and
says so; item *type* names are matched case-insensitively in `items_of_type`, because MSBuild item
names are, and treating `<Clcompile>` as a different type would silently drop a source file.

**Values are trimmed, conditions are not.** MSBuild preserves whitespace inside a property value,
so trimming is a small infidelity. It was chosen because Visual Studio never writes significant
leading or trailing whitespace, hand-formatted sheets do wrap long values across lines, and a
trailing newline inside a directory-valued property corrupts every path concatenated from it — a
failure that surfaces as a missing include directory rather than as anything legible. Semicolon
lists get trimmed per item downstream in any case. Conditions keep their padding, because MVP-09
owns what a condition means and the reader has no business normalizing an expression.

Values are read by concatenating a node's *immediate* text children rather than calling
`Node::text()`, which returns only the first and would silently truncate a value a comment happens
to split.

**Two policies for elements the reader does not model.** Skipping everything unmodelled was
rejected: `<Choose>`/`<When>` decides which properties apply, so dropping it produces a plausible
compilation database that is wrong, which CLAUDE.md §3 calls out as the worst outcome. Recording
everything unmodelled and warning about all of it was also rejected: `<Target>` and
`<ProjectExtensions>` appear in ordinary project files and cannot affect how a source file
compiles, so warning about them would train the user to ignore warnings — the same argument MVP-05
used for skipping solution folders in silence.

So there are two lists. A short set of known-irrelevant elements is skipped without comment, and
everything else is recorded as `Element::Unsupported` *and* warned about. Recording it as well as
warning means MVP-13 can revisit the wording without the reader having thrown the fact away.

The cost is that `Element::Unsupported` is dead weight until MVP-13 reads it, and that the
known-irrelevant list is a judgement call that will need revisiting the first time a real solution
puts something load-bearing in a `Target`.

**A missing file is `Io`, not `MissingImport`.** `Error::MissingImport` carries a `referenced_by`,
and the reader does not know its referrer — it may not have one. MVP-10 checks for existence before
calling in, so the error that names both files is raised by the code that knows both.

**`ProjectSchema` is a new wire kind rather than a reuse of `ProjectParse`.** `Error::ProjectParse`
carries a `roxmltree::Error`, which cannot be fabricated for a file that parsed perfectly well and
simply is not a project, so a new `Error` variant was needed regardless. The narrower option was to
map it onto the existing `ErrorKind::ProjectParse` — no new string in the plugin contract, on the
grounds that ADR 02 keeps kinds coarse and both failures mean "this file is broken."

A distinct kind was chosen because the two are not the same thing to a user. `project-parse` means
the file is corrupt. `project-schema` means the file is intact and is not a C++ project — which is
what happens when a `.vcxproj.filters`, a `.csproj`, or an unrelated XML file reaches the tool, and
the plugin can say so specifically. They share exit code 4, since a shell only needs the class.
The cost is one more published string that can never change, for a case that may prove rare.

**The reader is not where hostile input is stopped, and right now nothing else is either.** The
lowering is iterative by construction — every helper walks one level of children and none recurses,
so the depth of the document cannot grow the stack. `roxmltree::Document::parse` is a different
matter: it recurses once per nesting level, and a debug build on a 2 MiB thread stack survives 100
levels but dies at 200, from roughly 1.5 KB of XML. The process aborts outright, so there is no
error, no exit code, and nothing for the plugin to surface — precisely the failure mode CLAUDE.md
§5 forbids.

`roxmltree`'s `ParsingOptions` do not fix it. `allow_dtd` is already `false` by default, which is
why a DTD is rejected and the entity-expansion surface is closed, and it is deliberately left at
the default. But `nodes_limit` caps the *number* of nodes, not their depth, and cannot separate
5,000 nested elements from a large but ordinary project file — real solutions have thousands of
items. The guard has to bound depth before the text reaches the parser, which is its own piece of
work: **MVP-35**. The test for it is written and `#[ignore]`d, since running it takes down the
whole test binary.

> **Resolved by MVP-35.** `parse_project_text` now bounds nesting with a byte scan before the text
> reaches `roxmltree`, and the test runs. See [ADR 14](14-xml-nesting-depth-guard.md).

## Consequences

- **MVP-08 and MVP-09 consume `Setting`**, expanding `value` and evaluating `condition`, and must
  handle a group condition and a child condition as two separate gates.
- **MVP-10 calls `parse_project` per import** and owns `Error::MissingImport`, checking existence
  before calling in so the referrer can be named. It also owns the cycle and depth caps for the
  *import graph*, which are separate from MVP-35's caps on a single document.
- **MVP-12 consumes `items_of_type("ClCompile")`** and the `ItemDefinitionGroup` metadata beside
  it, and inherits `Item::update` for per-file settings.
- **MVP-13 inherits `Element::Unsupported`** and the reader's existing warning wording, including
  the two cases it already warns about that are arguably its business: an item naming no `Include`,
  `Remove` or `Update`, and a non-`Import` child of an `ImportGroup`.
- **MVP-35 is new**: bound XML nesting depth before parsing, unblocking
  `a_deeply_nested_document_is_rejected_rather_than_crashing`. **Done** — a 32-level cap enforced
  by a byte scan; see [ADR 14](14-xml-nesting-depth-guard.md).
- **`ErrorKind::ProjectSchema` is now part of the plugin contract** and cannot be renamed.
- **New open question:** nothing expands a wildcard `Include`. `<ClCompile Include="src\*.cpp"/>`
  is legal MSBuild; Visual Studio never writes it, so it has stayed invisible, but MVP-12's scope
  does not mention globbing either. A project written that way currently yields no source files at
  all, silently. Whether to expand wildcards or to warn loudly and refuse is unsettled.
- **New open question:** a property value containing element children keeps the text and drops the
  elements. It is not valid MSBuild, so the exposure is small, but it is the one place this design
  drops something without saying so.
