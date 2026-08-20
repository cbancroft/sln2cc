# ADR 49 — Config-Based Solution Pinning

- **Status:** Accepted
- **Date:** 2026-08-16
- **Issue:** [V2-02 — Config-based solution pinning](../todos.md#v2) (`feat/solution-pin`)
- **Affects:** `nvim/lua/sln2cc/config.lua`, `nvim/lua/sln2cc/discovery.lua`,
  `nvim/lua/sln2cc/commands.lua`, `nvim/lua/sln2cc/health.lua`,
  `nvim/tests/test_discovery.lua`, `nvim/tests/test_commands.lua`,
  `nvim/tests/test_health.lua`, [project_spec.md](../project_spec.md) ("Open Questions"),
  [engineering_spec.md](../engineering_spec.md) ("Neovim Plugin — Responsibilities"),
  [ADR 44](44-drop-cli-side-discovery.md), [ADR 45](45-nvim-solution-picker.md),
  [ADR 48](48-nvim-vanished-solution-recovery.md)

## Problem

`project_spec.md`'s Open Questions has carried this since the document's early drafts: "Should
there be a way to explicitly pin/override the active solution via config (e.g. for CI or
scripting use), bypassing the interactive picker?" `milestones.md` scopes the answer for V2:
"Config-based override to explicitly pin the active solution... bypassing the interactive
picker." Nothing answered it: `discovery.lua`'s `M.active_solution(root)` only ever produces a
persisted choice or the first-discovered `.sln`, and every command that needs a solution
(`commands.lua`'s `resolve_solution`) can end up opening `picker.lua`'s `vim.ui.select` — fine
interactively, useless in a CI job or a headless script with nothing to answer the prompt.

## Decision

1. **The pin is checked inside `discovery.active_solution`, before the persisted/discovered
   fallback**, not in `commands.resolve_solution` or anywhere else. `active_solution` is already
   the single source of truth for "what solution is active" — ADR 48 established that principle
   for vanished-solution detection, and the same reasoning applies here: teaching a second module
   to also know about the pin would let `commands.lua` and `picker.lua` (which independently
   calls `active_solution` for its "(active)" marker) disagree about what's active. When
   `config.options.pinned_solution` is a non-empty string, it wins unconditionally — the
   persisted-choice/first-found branch is never reached at all while a pin is configured.

2. **`M.active_solution` gains a third return value, `pin_error`, distinct from the existing
   `vanished`.** A configured-but-missing pinned path is reported through `pin_error`, not
   `vanished`, because the two must be handled differently downstream:
   - `vanished` (ADR 48) means "a *persisted* choice used to exist and no longer does" and
     recovers by rescanning and reopening the picker — a reasonable recovery for an interactive
     session, where a human is present to answer `vim.ui.select`.
   - `pin_error` means "the operator told us exactly which solution to use, and that file isn't
     there." A pin exists specifically so CI/scripting callers never see a picker; routing this
     into the same rescan-and-reprompt flow would silently block forever on a prompt nothing can
     answer, in the one context this feature was built for. `commands.resolve_solution` checks
     `pin_error` first and stops with a `vim.notify` at `ERROR`, the same "loud, no guessing"
     shape every other CLI/config failure in this plugin already uses — no rescan, no reprompt.

3. **The config default is `pinned_solution = ""`, not `nil`.** `config.lua`'s `validate()`
   treats any key present in user `opts` but absent from `M.defaults` as an unknown option, so an
   optional string field needs a same-typed default to pass its own validation. Empty string is
   the natural "unset" value for a string option and needs no special-casing beyond a plain
   `pinned ~= ""` check.

4. **The pinned path must be an absolute path to the `.sln` file, resolved against nothing.**
   Every path this plugin hands to the CLI is already documented as absolute (`cli.lua`'s
   `solution` parameter throughout); a pin is aimed at scripts/CI that already know the exact
   path they want, so there is no root-relative resolution to silently guess at.

5. **`:Sln2ccSelectSolution` needed no change.** It calls `picker.pick_solution` directly, never
   `discovery.active_solution`'s persisted-choice branch, so a manual pick still "succeeds" and
   persists via `set_active_solution` even while a pin is configured — but the very next call to
   `active_solution` (including that same command's own "(active)" marker on a later invocation)
   sees the pin again regardless, since Decision 1 makes the pin check unconditional. This is a
   direct consequence of where the check lives, not a new branch anywhere else.

## Discussion

**Why not fold the check into `commands.resolve_solution` instead.** That was the alternative
seriously considered: it would keep `discovery.lua`'s persisted/discovered logic untouched and
put all pin-awareness in one call site. Rejected because `picker.lua`'s `pick_solution` already
calls `discovery.active_solution` directly to mark the currently-active entry in its list — if
the pin lived only in `commands.lua`, that marker would silently disagree with what
`resolve_solution` actually resolves to whenever a pin is set, and any future direct caller of
`active_solution` would need to redundantly reimplement the same check to get a correct answer.
Keeping "what's active" a single-module concern (the same argument ADR 48 made) avoided that.

**Why a third return value instead of overloading `vanished`.** The CI/no-reprompt distinction
*is* the entire reason this issue exists: if a missing pin degraded to the same recovery as a
vanished persisted choice, the feature would not actually bypass the picker in the one scenario
it was built for. A separate, explicitly-named return value keeps that distinction visible at
every call site rather than requiring a caller to infer "this vanished path came from a pin, so
don't reprompt" from some other signal.

**Why not validate the pin path is absolute.** Considered and rejected as unnecessary
enforcement: a relative pin simply won't resolve to an existing file relative to Neovim's cwd in
the general case, so it surfaces as `pin_error` (a clear, actionable notify) rather than silently
doing the wrong thing. Adding a separate "is this absolute" check would be validating a symptom
CLAUDE.md's "never silently guess" is already covering structurally.

## Consequences

- Resolves `project_spec.md`'s Open Question: a config-based pin now exists, checked ahead of any
  persisted or discovered choice, and a missing pinned file is a hard, non-interactive error
  rather than a silent fallback or a picker prompt nothing can answer.
- `discovery.active_solution`'s signature grows from two return values to three. Every existing
  direct caller (`picker.lua`, `commands.lua`, `health.lua`) truncates or reads the extra value
  safely; no call site needed a structural change beyond `health.lua`'s own diagnosis (below).
- `health.lua`'s `check_solution` also reads `pin_error` now, reporting it as a `vim.health.error`
  naming the missing pinned path instead of the generic "no `.sln` found under `<root>`" warning
  — a pinned-but-missing solution is a materially different, sharper diagnosis (the tree may
  contain solutions just fine; the *pinned one specifically* is gone), and `:checkhealth` is
  exactly where CLAUDE.md §4 says a misconfiguration like this should surface legibly.
- Changelog entry added: a solution can now be pinned via `pinned_solution` config, bypassing the
  picker entirely (for CI/scripting).
