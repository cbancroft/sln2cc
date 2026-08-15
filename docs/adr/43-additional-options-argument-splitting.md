# ADR 43 — `AdditionalOptions` Argument Splitting

- **Status:** Accepted
- **Date:** 2026-08-15
- **Issue:** [MVP-38 — Shell-style `AdditionalOptions` splitting](../todos.md#msbuild-evaluation)
  (`fix/additional-options-splitting`)
- **Affects:** `cli/src/msbuild/flags.rs`, [engineering_spec.md](../engineering_spec.md)
  ("Diagnostics", Open Questions), [ADR 13](13-diagnostic-severity-and-wording.md)

## Problem

`flags.rs::compile` puts `AdditionalOptions` last, verbatim, so a hand-written switch can override
anything the flag table produced — but MVP-12 split it on whitespace, which is right for every
switch a project realistically writes there and wrong for exactly one shape: a quoted argument
containing a space. `/DBANNER="hello world"` becomes two flags the project never wrote,
`/DBANNER="hello` and `world"`, and `/I"C:\Program Files\x\include"` points clangd at two
directories that do not exist. MVP-13 declined to fix this and made it *visible* instead — a value
containing `"` now warns that it will reach clangd as separate flags — because the fix is not the
splitter itself but the decision underneath it: MSBuild's own quoting rules, `cl`'s response-file
rules, and the Windows command-line parser (`CommandLineToArgvW`) disagree, most visibly about
backslashes preceding a quote, and `.vcxproj` files are written by people who tested against `cl`
rather than against a spec.

## Decision

**Implement the Microsoft C runtime / `CommandLineToArgvW` backslash-quote algorithm** — the same
rules `cl.exe`'s own command-line parsing and clang-cl's `TokenizeWindowsCommandLine` use. A private
`split_arguments` in `flags.rs` replaces the whitespace split and the `"` warning outright; there is
no fallback path and no flag to choose the old behavior.

The algorithm:

- Arguments are whitespace-delimited outside a quoted region.
- A `"` toggles whether whitespace splits.
- A run of `N` backslashes immediately followed by `"` contributes `N / 2` literal backslashes; if
  `N` is odd, the trailing backslash escapes the quote into one literal `"` character and quoting
  state is unchanged; if `N` is even, the `"` instead toggles quoting state.
- A run of backslashes *not* followed by `"` is copied through literally.
- An unterminated quote is not an error — the argument runs to the end of the string, whitespace
  included. Untrusted `.vcxproj` input must never hang or drop content because of it.

MVP-13's warning is removed: the condition it existed to flag — "the split will silently corrupt
this" — no longer holds. `flags.rs` keeps its other `AdditionalOptions` check (an unresolved `$(`/
`%(` reference) unchanged; that is a different failure mode from tokenization.

## Discussion

**Why the Microsoft argv algorithm and not MSBuild's own rules.** The premise of "MSBuild's
quoting rules" turned out not to be a real alternative once traced through. `AdditionalOptions` is
inserted verbatim onto the actual command line (or response file) MSBuild's `CL` task hands to
`cl.exe` — MSBuild never retokenizes the string itself. What MSBuild has is *value* escaping for its
own XML/property syntax (`%3B` for a literal `;`, and so on), which governs how a `.vcxproj` author
writes the string, not how the compiler consumes it. So the real fork was always between `cl.exe`'s
parsing and something else, and `cl.exe`'s parsing *is* the Microsoft C runtime algorithm — the same
one `CommandLineToArgvW` implements for any Windows program's `argv`.

**Why this also settles the clangd side for free.** CLAUDE.md's architecture notes that flags stay
in MSVC form and are consumed by clangd in clang-cl mode rather than being translated. clang-cl's own
command-line tokenizer (`TokenizeWindowsCommandLine` in LLVM's `Support/CommandLine`) implements this
exact algorithm for MSVC compatibility — it has to, since clang-cl exists to be a drop-in for `cl.exe`
on real build systems. One algorithm therefore matches both ends of the pipe: what `cl.exe` would
have done with this text, and what clangd will do with the flags msvclsp hands it. There was no
tiebreak to make between "whose rules" beyond confirming they're the same rules.

**Why not POSIX-shell-style splitting.** The tempting alternative is a `shlex`-shaped splitter,
since it is a more familiar algorithm and Rust's ecosystem has crates for it. It was rejected because
it answers a different question: POSIX shells treat a bare `\` before nearly any character as an
escape (including before a space, which the MS algorithm does not — a lone backslash there is
literal), and single-quote semantics differ from double-quote semantics in ways `AdditionalOptions`
text has no way to signal. Implementing shell rules against Windows-authored, `cl`-consumed text
would produce a splitter that is internally consistent and wrong for this input.

**Why no fallback or feature flag.** The whitespace-only case — no `"` or `\` in the value — is a
strict subset of what the new algorithm does: it splits on runs of whitespace and touches nothing
else, identically to `split_whitespace()`. There is no behavior to preserve behind a flag; every
existing project without quoting sees the same output it always did, which is what
`additional_options_without_quotes_splits_on_whitespace_as_before` pins.

**Why the splitter is a private free function in `flags.rs` rather than a new module.** It has no
MSBuild vocabulary in it — it is pure string tokenization — and nothing outside `compile` needs it,
the same reasoning that already places `list`, `boolean`, and `dedup` beside it rather than in a
shared utilities module.

## Consequences

- **MVP-13's `AdditionalOptions` warning is gone.** The engineering spec's Diagnostics catalogue
  loses the row that named it, and the corresponding case in
  `13-diagnostic-severity-and-wording.md` is marked resolved in place, per the house rule that a
  superseded ADR keeps its original argument.
- **The engineering spec's open question on argument splitting is resolved and removed.**
- **A quoted argument's content, once unquoted, is passed through unexamined.** If a project writes
  `/DMSG="`, an odd number of literal backslashes before end-of-string, or any other malformed
  shape, `split_arguments` still terminates and returns *something* rather than erroring — matching
  `cl.exe`'s own tolerance for a command line it did not validate either.
- **No new dependency.** The algorithm is under thirty lines of `char` iteration; there was nothing
  in `Cargo.toml`'s settled dependency list (CLAUDE.md §5) that offered it directly, and pulling one
  in for a self-contained state machine this small would not clear the bar the existing dependency
  decisions set.
