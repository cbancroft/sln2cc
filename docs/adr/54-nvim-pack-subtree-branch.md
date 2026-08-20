# ADR 54 — Publishing the Neovim Plugin via a Subtree-Split Branch

- **Status:** Accepted
- **Date:** 2026-08-20
- **Issue:** [MVP-48 — Publish the Neovim plugin as a `vim.pack` target](../todos.md#mvp-48)
  (`feat/nvim-pack-subtree`)
- **Affects:** `.github/workflows/ci.yml` (new `publish-nvim-plugin` job), `README.md`
  ("Using the Neovim plugin"), a new orphan-history branch `nvim-plugin`

## Problem

Neovim's built-in package manager, `vim.pack.add()`, clones a git ref and adds exactly that
clone's root directory to `runtimepath`. Its spec table takes `src` and `version` (a git
ref/tag/branch) — there is no `dir` or subdirectory option comparable to `lazy.nvim`'s `dir =`.
`plugin/sln2cc.lua`, `lua/sln2cc/`, and `doc/sln2cc.txt` all live under `nvim/`, a sibling of
`cli/`, per the split this project deliberately maintains (CLAUDE.md §2: "Two independent
artifacts communicating through the filesystem and a subprocess boundary"). Pointing
`vim.pack.add` at this repo's `main` branch would put `nvim/plugin/`, not `plugin/`, on
`runtimepath` — Neovim would clone the repo and load nothing.

## Decision

Keep `main` and the `cli/`/`nvim/` layout exactly as they are. Add a CI job,
`publish-nvim-plugin`, that runs after the `nvim` test job passes on a push to `main` touching
`nvim/**`, and does:

```sh
git subtree split --prefix=nvim -b nvim-plugin
git push origin nvim-plugin --force
```

`nvim-plugin`'s tree root is then exactly `nvim/`'s current contents — `plugin/`, `lua/sln2cc/`,
`doc/`, `tests/` — with history rewritten so each commit corresponds to a commit that touched
`nvim/` on `main`. Users install with:

```lua
vim.pack.add({ { src = "<repo-url>", version = "nvim-plugin" } })
```

## Discussion

**Rejected: promote `nvim/`'s contents to the repository root.** This would make `main` itself
directly `vim.pack`-installable with no extra branch or CI job. It was rejected because it
costs more than it looks like: it rewrites the tree diagram in CLAUDE.md §2, the CI workflow's
existing job structure (which already treats `cli/` and `nvim/` as parallel, independently
testable trees), and both `.claude/skills/start-issue` and `review-issue`, which assume the
current layout when reasoning about which paths an issue touches. It also weakens the "two
independent artifacts" framing CLAUDE.md leads with — a repository root that *is* the Neovim
plugin, with the CLI merely nested inside it, reads as the plugin being primary and the CLI
secondary, which inverts the project's actual center of gravity (the CLI is the harder, larger
artifact; MVP work has been overwhelmingly CLI work). The subtree-split branch achieves
installability without any of that cost, at the price of one CI job.

**Rejected: split into two repositories.** Cleanest fit for Neovim-plugin-ecosystem convention
(a plugin repo is normally single-purpose), and sidesteps subtree tooling entirely. Rejected for
now because it requires deciding a cross-repo versioning story — does a CLI release imply a
plugin release, can they drift independently, how does an issue that touches both land as one
change — before there is a published release of either to force that decision. Revisiting this
is explicitly left open below; nothing about the subtree-branch approach forecloses it later.

**Force-push to a dedicated branch, not a normal merge.** `nvim-plugin`'s history is
synthetically rewritten by `git subtree split` on every run, so its commit hashes are not stable
across CI runs even when `nvim/`'s content hasn't changed relative to the previous run's parent
context. Treating it as an ordinary branch that accumulates commits would fork its history from
`main`'s in a way nothing could later reconcile. Force-pushing keeps it a pure, disposable
projection of `nvim/`'s current state — nothing should ever be based on `nvim-plugin` directly
or expect its history to be stable.

**Path-filtered and gated on the test job, not run on every push.** Running the split on every
CI push regardless of whether `nvim/` changed would force-push an identical branch on unrelated
`cli/`-only commits, which is just noise on the remote. Gating on the `nvim` job's success means
a broken plugin (failing `mini.test` specs) never reaches the branch users install from — CI
enforces at least the same bar for the published branch as for `main`.

## Consequences

- Installing via `vim.pack` requires knowing about `nvim-plugin` specifically; this needs to be
  documented prominently (done: README.md's "Using the Neovim plugin" section) since it is not
  discoverable from the branch name alone without reading the docs.
- `nvim-plugin` must never be treated as a normal development branch — no direct commits, no
  PRs targeting it, nothing based on it. It exists only as CI's force-pushed projection of
  `nvim/`.
- If `nvim/`'s directory name or internal layout ever changes, `--prefix=nvim` in the CI job
  needs to change with it — nothing enforces that link automatically.
- **Open question, deliberately unresolved here:** whether to revisit the two-repository split
  once there's a first tagged release of either artifact, at which point a real versioning
  story becomes unavoidable regardless of which repo layout is chosen.
