---
name: start-issue
description: Start work on a numbered issue from docs/todos.md (MVP-14, V1-03, …) — create its branch, plan the implementation for review, then write the tests and documentation while the user writes the code. Use when the user says they want to start working on an issue by its ID. Use review-issue for the review and commit once they have written it.
---

# Starting an issue

`docs/todos.md` stands in for a GitHub issue tracker. An issue ID looks like `MVP-14`, `V1-03`,
`V2-02`. The user supplies one; everything below hangs off it.

`review-issue` picks up from here once their implementation exists.

## Division of labour — read this first

**The user writes the implementation. You write the tests and the documentation.** This governs
every phase and is easy to violate by reflex. Do not edit anything under `cli/src/**` outside
`#[cfg(test)]` modules, or anything under `nvim/lua/**`, unless the user asks for a specific change
— at which point make exactly that change and nothing adjacent.

**Why**, because a rule without its reason is one you will rationalise around on a hard issue: the
user is learning Rust on this project, wants first-hand knowledge of what is in the codebase, and
wants to adjust an implementation as they go rather than describing a tweak and hoping an agent
reads it the way they meant.

That sets the bar for the plan's appendix. It is a worked reference to react to, not a fait
accompli — so it has to be worth reading *and worth disagreeing with*. Spell out the reasoning in
its doc comments and name the trade-off you took. Code that merely compiles is a failure here.

You deliver: the branch, a plan, the tests, the ADR, the spec updates, the changelog entry, and the
`todos.md` outcome.

## Phase 1 — Locate and branch

1. Read the issue's entry in `docs/todos.md`. The line under the title carries a **branch name**
   (`feat/…`, `fix/…`) and an estimate. **Use the recorded branch name**; do not invent one.
   **Ignore the estimate** — it is the user's own figure for implementing without AI assistance and
   says nothing about your scope. Do not plan around it or report against it.

2. **Check whether this issue is already underway** before creating anything. Invoking the skill
   twice on the same issue is a normal accident, not an error state:
   - `git branch --list <recorded-name>` — if it exists, check it out instead of creating it.
   - `.plans/<ISSUE-ID>.md` — if it exists, read it. That is the plan of record. Do not re-plan
     from scratch; the user has already approved it.
   - `git log main..<branch>` and `git status` to see what landed.

   Then say what you found and what is outstanding, and resume from the first unfinished phase.
   Re-planning approved work silently is worse than asking.

3. Otherwise `git checkout -b <recorded-name>` from `main`, and confirm the checkout in your reply.

4. Establish the baseline before touching anything — `cargo test` from `cli/` — so any later failure
   is unambiguously yours. Note the pass/ignore counts.

## Phase 2 — Research before planning

The issue entry is not the whole brief. Do all of these:

- **`grep` the issue ID across `docs/`.** Completed items carry an **Outcome** paragraph, and those
  routinely hand work forward: "MVP-13 inherits six more wordings", "MVP-12 owns wiring it up",
  "this shrinks MVP-13 to…". This is usually where most of the real scope lives, and it is the step
  most easily skipped.
- Read the ADRs the entry links, plus any ADR whose **Consequences** name this issue.
- Read the `#[cfg(test)]` blocks of the modules involved, not just the code. Existing tests encode
  decisions the prose does not, and their doc comments are where the reasoning usually is.
- Check **Open Questions** in `docs/engineering_spec.md` and `docs/project_spec.md`. Per CLAUDE.md
  §9, if the task touches one, raise it rather than silently picking an answer.
- Note the entry's **Tests:** block. That is the issue's own test specification, and Phase 4 is
  accountable to it.

## Phase 3 — Plan

Enter plan mode. Use `AskUserQuestion` for decisions that are genuinely the user's: an unresolved
spec Open Question, a wire-format change, a dependency, anything where two readings produce
materially different work. Recommend, don't just survey.

Write the plan to the harness plan file. That file is the *only* thing you may write during plan
mode, so the copy has to wait: **after `ExitPlanMode` is approved, copy it to
`.plans/<ISSUE-ID>.md`** — gitignored, predictable, and still there after a session clear. Tell the
user both paths; they ask every time.

The plan must contain:

- **Context** — the problem, and what in the repo makes it a problem now.
- **Decisions**, each with its reasoning, since these become the ADR.
- **Files to change**, as a table.
- **Tests** you will write, by name and file.
- **Documentation** you will write.
- **Verification** — the full pre-push check from CLAUDE.md §7.
- **Appendix: reference implementation.** Code sketches in the house style, with real doc comments.
  This is the point of the exercise: the user reads it, adopts or rejects it, and writes the actual
  code. Say plainly that it is a sketch you will not apply.

Flag anything in the issue's **Tests:** block that cannot be written as specified, and say why.

## Phase 4 — Tests and documentation

### Tests

Write them against the appendix's API. **They will not compile until the user's code lands.** Say so
up front — red is the expected state here, not a mistake.

What makes red trustworthy is proving it is *only* the expected red: run `cargo check --all-targets`
and confirm every error names a missing item from the appendix and nothing else. Report that list —
it doubles as the user's to-do list. Then run `cargo fmt` so your own additions are clean.

Match the surrounding style: a doc comment saying *why the case matters*, not what it does. Include
negative cases, so a check cannot be satisfied by always firing.

Constraints from CLAUDE.md §8 that bite hardest on the toolchain and fixture issues:

- Integration tests inject `FakeToolchain`. A test that depends on a real Visual Studio installation
  is a broken test — it cannot run in CI and will not reproduce across machines.
- **Never** regenerate golden files against a real installation. They come from `FakeToolchain`
  output, and a golden captured from a live toolchain is unreviewable and machine-specific.
- Plugin tests stub the CLI at the `cli.lua` boundary and never invoke the binary or require a
  Windows toolchain.
- A test that cannot be written yet becomes an `#[ignore]`d placeholder (or `pending()` in Lua)
  naming the issue that unblocks it. The gap stays visible rather than being left out.

### Documentation

All of it in this same change, per CLAUDE.md §9:

| Where | What |
| --- | --- |
| `docs/adr/NN-<3-6-word-descriptor>.md` | Next unused number, never reused. Problem / Decision / Discussion / Consequences. The **discussion** is the part with value later: options rejected and why, and what the decision costs. Must reference the todo item. |
| `docs/engineering_spec.md` | Current-state description, pointing at the ADR for the argument. Remove any Open Question this resolved; add any it surfaced. |
| `docs/changelog.md` | `[Unreleased]`, written for someone *using* sln2cc — what changed for them, not which functions moved. Skip purely internal work. |
| `docs/todos.md` | Tick the item, add its **Outcome** paragraph in the established voice, and record any test that could not be written as specified. |
| Superseded ADRs | Mark the resolved question in place. Never rewrite or delete one. |

Add a new todo item for anything deliberately deferred, and point the relevant spec Open Question at
it. Update the tree in CLAUDE.md §2 if modules moved, and `README.md` if commands changed.

The ADR and the Outcome name functions and types that do not exist yet. That is fine — they are
**drafts**. `review-issue` reconciles them against the finished code before anything is committed.
