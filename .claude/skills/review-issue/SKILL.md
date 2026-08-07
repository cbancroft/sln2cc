---
name: review-issue
description: Review the user's implementation of a docs/todos.md issue, reconcile the drafted ADR and spec against the code that actually landed, and commit. Use when the user asks you to review their implementation, review the current branch, or commit an issue's work. The companion to start-issue, which produced the branch, tests, and draft documentation.
---

# Reviewing and landing an issue

`start-issue` created the branch, wrote the tests, and drafted the documentation against a
*reference* implementation. The user then wrote the real one. This is where the two are reconciled
and the work lands.

The plan is at `.plans/<ISSUE-ID>.md` (gitignored). Read it first — it holds the decisions the
review is measuring against.

**You still do not edit their code**, and this is the phase where the temptation is worst — the
defect is in front of you and the fix is two lines. Report it anyway. The user is learning Rust here
and wants to know what is in the codebase first-hand; a fix you applied is one they did not make a
decision about. Change something only when they ask for that specific thing, and then only that.

Your own tests and documentation remain yours to fix, and section 3 requires it.

## 1 — Run everything before reading anything

The full pre-push check from CLAUDE.md §7, from `cli/`:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

and the plenary suite from `nvim/`. All four are CI gates, so a failure in any is the headline of
your review regardless of what the code looks like.

Formatting deserves a specific note: when `rustfmt` reports `left behind trailing whitespace`, the
trailing space is the *symptom*. The cause is almost always a string literal pushing its call past
`max_width`, so rustfmt declines to touch the expression and leaves the original text. Point at the
long line, not the whitespace — `cargo fmt` alone will not fix it.

Confirm the new tests actually **run**, not merely compile: grep the test output for their names. A
missing `#[test]` is invisible in a green run.

## 2 — Review the implementation

Read the diff against the plan. Report findings ordered by severity, and separate what blocks a
commit from what is a judgment call from what is a nit.

Beyond ordinary correctness, this repo's recurring failure modes:

- **Doc comments that no longer describe the code.** A renamed or re-scoped function usually keeps
  its old comment, and in a codebase where the comments carry the reasoning that is a real defect.
- **Claims in a doc comment that are narrower or wider than the behaviour** — "these are squelched"
  on a variant when the squelching applies to all of them.
- **Public items with no caller outside their module**, especially where a rename removed the reason
  the visibility existed.
- **Every call site versus the catalogue or table the docs added.** Enumerate them and check.
- **Behaviour that drifted from the plan's decisions** — not automatically wrong, but it has to be
  deliberate, and if it is, the ADR needs to say so.
- **A message or classification that points the wrong way** for one of its call sites. A shared
  wording that is right in three places and backwards in the fourth is worse than two wordings.

## 3 — Reconcile your own work

The tests, the ADR, the spec, and the `todos.md` Outcome were all written against the *reference*
implementation. The real one may differ. Everything in this section is yours to fix — do it rather
than reporting it.

**The tests.** If the user renamed or reshaped an API, update the tests to match their code. Do not
ask them to rename something so your tests keep compiling; the implementation is the thing being
specified, and your tests follow it. What you may not do is weaken an assertion to make a failure go
away — if a test now fails on behaviour rather than on names, that is a review finding, not a test
to soften.

**The prose.** Re-read all three documents against the code that exists now. This has produced real
drift before, so treat it as a checklist rather than a glance:

- Every function, type, method, and variant they name — does it still exist under that name?
- Every catalogue row, table entry, and enumerated behaviour — does it match the call sites?
- Does the changelog describe what the code now does, for a user?
- Did the implementation surface a decision the ADR does not discuss? Add it, including anything the
  user chose *not* to fix — a known limitation written down is worth more than a silent one.

**The issue's own test specification.** Re-read the **Tests:** block in the `docs/todos.md` entry and
account for every line of it: covered, or excused in writing under the item. This is the last point
at which a required test can quietly go missing.

## 4 — Commit

Re-run the full check, then stage and commit.

One focused commit unless the user asks otherwise. Imperative summary line naming what changed, not
the issue ID — read `git log -3` for the register; the bodies are long, and they explain the
*decisions* and what each cost rather than listing files. Cover what was deliberately not done and
why. End with:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

Never commit to `main` directly. Merging afterwards is the user's call — this repo has a linear
history and no remote, so `git merge --ff-only` is the convention; a merge commit would be its
first. Leave the branch in place unless they ask you to delete it.
