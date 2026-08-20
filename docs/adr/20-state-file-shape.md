# ADR 20 — State File Shape and Root-Relative Solution Keys

- **Status:** Accepted
- **Date:** 2026-08-10
- **Issue:** [MVP-18 — State file shape and store initialization](../todos.md#persistence) (`feat/db-schema`)
- **Affects:** `cli/src/db.rs`, [engineering_spec.md](../engineering_spec.md) ("Persistence",
  "Storage Layout", Open Questions), [ADR 19](19-hashed-storage-directory-write.md), MVP-19, MVP-20

## Problem

`db.rs`'s `Store` has been fully stubbed (`open`, `set_active_config`, `active_config`,
`is_up_to_date`) since MVP-01 reshaped this issue from a SQLite schema into a JSON state file
(ADR 01). Every body is `todo!()`. `engineering_spec.md`'s Open Questions list has carried
"State file shape (keys for solutions, configurations/platforms, file timestamps) and how
solution identity is keyed so it survives the working directory changing" since then, and
MVP-19 (active config persistence) and MVP-20 (mtime staleness tracking) can't be scoped
without an answer: both read and write through whatever shape this issue settles.

## Decision

```rust
const CURRENT_VERSION: u32 = 1;

struct State {
    version: u32,
    solutions: BTreeMap<String, SolutionRecord>,
}

struct SolutionRecord {
    known_configs: Vec<ConfigPlatform>,
    active_config: Option<ConfigPlatform>,
    files: BTreeMap<String, i64>,
}

impl Store {
    pub fn open(sln2cc_dir: &Path) -> Result<Self>;
    fn solution_key(&self, solution: &Path) -> String;
}
```

- **A single file, `.sln2cc/state.json`**, not one file per solution. The state file has to
  track *which* solutions exist and which one is active, which a per-solution file would need
  a separate index for anyway — the single file gets that for free, matching
  `engineering_spec.md`'s own "likely a single file... since it needs to track which solution
  is currently active."
- **Solutions are keyed by their `.sln` path *relative to the workspace root*, not an absolute
  path.** `engineering_spec.md` said "absolute path" until this ADR; that goes stale the
  moment the whole workspace relocates (a clone to a new path, a drive letter change, a
  different CI checkout path) — every key silently stops matching, with no error, and the
  cache just looks empty. Root-relative keys survive that. `Store` derives its own root once,
  at `open()` time, as `sln2cc_dir.parent()` — "Storage Layout" already guarantees `.sln2cc`
  sits directly at the project root, so no new parameter is needed on `open()` or on the
  already-published `set_active_config`/`active_config`/`is_up_to_date` signatures, none of
  which take a root today.
- **A hand-rolled `..`-climbing relative-path diff**, not `Path::strip_prefix`. The CLI's own
  `--root` flag is documented to diverge from where the solution lives ("the plugin passes the
  root it detected from Neovim's LSP root markers, which often is not where the solution
  lives" — "CLI Surface"), so `strip_prefix` — which only succeeds when root is a literal
  prefix — would silently fall back to something for every solution not nested under root.
  Same-drive divergence (solution beside or above root) still produces a working relative
  path; only a genuinely unrepresentable case (a different Windows drive letter) falls back to
  storing that one entry's absolute path.
- **Key comparison and the fallback path are lowercased**, matching ADR 19's own reasoning for
  the hash-suffixed output directory: Windows paths are case-preserving but the filesystem is
  not, so an unlowered key would silently split one solution across two entries depending on
  invocation casing.
- **`version` is a hard gate, not a migration point, for v1.** An unrecognized version is
  `Error::StateVersion` (already defined in `error.rs`, unused since the enum was drafted) —
  nothing has shipped yet that a migration would need to read, so writing migration logic now
  is speculative. The field exists so a *future* version bump has somewhere to hook in.
- **`solutions` is a `BTreeMap<String, _>`, not a `HashMap`**, for deterministic serialization
  — matching the "no incidental nondeterminism" posture `compdb::build`'s own doc comment
  already commits the crate to elsewhere.
- **`known_configs` and `files` are reserved but unpopulated by this issue.** Nothing calls
  `Store::open` from the reachable CLI surface yet, and no accessor writes to either field —
  MVP-19 owns `active_config`, MVP-20 owns `files`. `known_configs` has no owning issue yet;
  see Consequences.
- **`Store` gains private `state: State` and `root: PathBuf` fields**, both set once at
  `open()` and held for the object's lifetime, rather than re-reading the file on every
  accessor call — the latter would make MVP-20's `is_up_to_date` pay a parse on every
  staleness check for no benefit, since nothing else writes the file concurrently within one
  process (this crate has already accepted last-writer-wins across *processes*; there is still
  only one writer *within* a process).
- **No new `Error`/`ErrorKind` variants.** `StateParse` and `StateVersion` were defined
  speculatively when the error enum was drafted (ADR 02) and are already wired through
  `kind()`, `exit_code()` (both exit code `4`), and the stable wire-string list. This issue is
  the first code path that actually returns them.

## Discussion

**Why not keep the spec's original "absolute path" wording.** It was never exercised by any
implementation — MVP-18 is the first thing to actually build the state file — so there was no
existing behavior to preserve, only a written intention to revisit. The relocation cost is
real and immediate: this project's own development moves between machines and directories
during its lifetime, and "the cache silently stops working after a `git clone` to a new path"
is a worse failure mode than the code needed to avoid it.

**Why not just always fall back to an absolute path when `strip_prefix` fails**, instead of
hand-rolling a full relative-path diff. `strip_prefix` failing doesn't mean *no* relative path
exists — it only means root isn't a literal prefix, which is true for every sibling or
ancestor of root, not just genuinely cross-drive cases. Given `--root` is documented to
regularly diverge from the solution's own location, "root isn't a literal prefix" is expected
to be the common case, not the exception — falling back to absolute for all of it would mean
relative keying buys almost nothing in practice for the solutions the divergence was written
about in the first place.

**Why hand-roll rather than take a `pathdiff`-style dependency.** Consistent with this crate's
existing precedent (`msbuild::paths::normalize`, `compdb`'s FNV-1a) and its stated dependency
policy ("prefer the standard library" — `Cargo.toml` comments, CLAUDE.md §5): this is the one
place the crate needs a relative-path diff, and it's a dozen lines over `Path::components()`.

**Why a single file rather than one per solution**, revisited from "Storage Layout"'s original
"likely." A per-solution file needs its own index of which solutions exist and which is
active — exactly what the single file already has to hold — so per-solution files would
duplicate that index rather than replace it, for no isolation benefit: `db.rs`'s own module
doc already accepts last-writer-wins with no locking, so splitting the file buys nothing on
the concurrency axis either.

**Why `version` rejects rather than migrates for v1.** A migration path is defensive code for
a scenario (an old-format file needing upgrade) that cannot occur yet — nothing has shipped a
version 0 or earlier. Building migration scaffolding now would be exactly the kind of
speculative generality CLAUDE.md §1 warns against ("don't design for hypothetical future
requirements"). The field is there so version 2 has a place to hook a real migration when one
is actually needed.

**One option considered and rejected: keying by the solution's filename stem alone** (the way
`compdb`'s `<solution-stem>-<hash>` output directory does, minus the hash). Rejected because
two solutions can legitimately share a stem in different directories — exactly the collision
`compdb`'s hash suffix exists to disambiguate (ADR 19) — and the state file has no equivalent
disambiguation mechanism to fall back on.

## Consequences

- Closes the `engineering_spec.md` Open Question this issue named. "Persistence" and "Storage
  Layout" now describe the finalized shape instead of "TBD."
- **`known_configs` has no owning accessor yet.** The shape reserves the field because the
  issue's own summary and `db.rs`'s pre-existing module doc both name it as part of what the
  state file stores, but no MVP issue currently populates or reads it — `list-configs`
  (MVP-21) doesn't cache its result today. Left as a gap rather than inventing scope: a future
  issue should either claim it or the field should be dropped if nothing ends up needing a
  cross-invocation cache of a solution's known configurations.
- **`Store::open` still isn't reachable from the CLI.** `generate`/`list-configs` don't wire
  persistence in until later issues (MVP-19 for `active_config`, MVP-22 for the `generate`
  pipeline), matching MVP-16/17's own precedent (ADR 18, ADR 19) of skipping the changelog for
  work not yet reachable from a real invocation.
- **`solution_key`'s cross-drive fallback is real but only exercisable on Windows.** Every
  absolute path on Unix shares a leading `/` root component, so the "no common prefix at all"
  branch has no portable non-Windows trigger — its test is `#[cfg(windows)]`, consistent with
  the crate's existing OS-conditional test helpers (`paths.rs`, `compdb.rs`'s own `absolute()`
  helper).
- **A solution moved independently of root still gets a new key**, same as the already-accepted
  "moved solution directory orphans its old `.sln2cc` output directory" cost ("Storage
  Layout"). Root-relative keying fixes *workspace* relocation, not *solution-within-a-stable-
  root* relocation — those are different costs, and only the first was in scope here.
