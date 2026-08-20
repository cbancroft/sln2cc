//! JSON-file persistence.
//!
//! Stores, per discovered solution: its known configuration/platform values, its own
//! last-used selection, and source/project file modification timestamps used to decide
//! whether a reparse is needed.
//!
//! State is scoped per-solution from the start. Only one solution is active at a time in
//! v1, but keeping the layout solution-scoped is what allows multiple simultaneously
//! active solutions later without a migration.
//!
//! SQLite was the original plan and was dropped: the payload is a few KB, and the only
//! practical way to link SQLite on Windows is to compile the bundled C amalgamation, which
//! would make a C toolchain a hard build requirement. The tradeoff accepted in exchange is
//! that concurrent writers are last-writer-wins rather than transactional — see
//! docs/engineering_spec.md. Writes must therefore be atomic (write a temporary file, then
//! rename over the target) so a crash or a concurrent run can never leave a torn file.
//!
//! The on-disk shape is settled — see ADR 20 (docs/adr/20-state-file-shape.md).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
// MVP-20 uses these to populate `SolutionRecord::files`; unused until then.
#[allow(unused_imports)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::msbuild::paths::normalize;
use crate::{ConfigPlatform, Error, Result};

/// Bumped whenever `State`'s shape changes incompatibly. v1 has no migration path. An unrecognized
/// version is a hard [`Error::StateVersion`], not an upgrade attempt since we don't have any other
/// versions yet.
const CURRENT_VERSION: u32 = 1;

const STATE_FILE: &str = "state.json";

/// The on-disk shape. `solutions` is a `BTreeMap` (not a `HashMap`) so serialization is fully
/// deterministic across runs.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    version: u32,
    solutions: BTreeMap<String, SolutionRecord>,
}

impl State {
    fn fresh() -> Self {
        State {
            version: CURRENT_VERSION,
            solutions: BTreeMap::new(),
        }
    }
}

/// Everything persisted for one solution, keyed in `State:solutions` by its path relative to the
/// workspace root (see `Store::solution_key`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct SolutionRecord {
    /// Configuration/Platform pairs recorded when this solution was parsed. Populated in a future issue.
    known_configs: Vec<ConfigPlatform>,
    active_config: Option<ConfigPlatform>,
    /// Normalized absolute path -> mtime (seconds since the Unix epoch).
    files: BTreeMap<String, i64>,
}

/// Handle to the state file stored under `.sln2cc/`.
#[derive(Debug)]
pub struct Store {
    pub path: PathBuf,
    /// The workspace root. This is `.sln2cc`'s own parent, and used to key solutions by
    /// root-relative path rather than absolute. See [`solution_key`].
    root: PathBuf,
    state: State,
}

impl Store {
    /// Open (creating if absent) the state file for a project root.
    /// A missing file is empty state, not an error. The state file is a cache, and an empty cache
    /// is a normal starting state. `.sln2cc` is created if it does not exist yet.
    pub fn open(sln2cc_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(sln2cc_dir).map_err(|e| Error::io(sln2cc_dir, e))?;
        let path = sln2cc_dir.join(STATE_FILE);

        // `sln2cc` always sits directly at the project root, so this is unreachable in practice.
        // Falling back to `sln2cc_dir` itself is a graceful degrade rather than an `unwrap()` on
        // a result nothing in this crate can construct without a parent.
        let root = sln2cc_dir.parent().unwrap_or(sln2cc_dir).to_path_buf();

        let state = match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let state: State =
                    serde_json::from_str(&contents).map_err(|source| Error::StateParse {
                        path: path.clone(),
                        source,
                    })?;
                if state.version != CURRENT_VERSION {
                    return Err(Error::StateVersion {
                        path,
                        found: state.version,
                        supported: CURRENT_VERSION,
                    });
                }
                state
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let fresh = State::fresh();
                write_state(&path, &fresh)?;
                fresh
            }
            Err(e) => return Err(Error::io(&path, e)),
        };

        Ok(Store { path, root, state })
    }

    /// Record the last-used configuration/platform for a solution.
    ///
    /// Overwrites any existing selection. Written through immediately (no batching). The state file
    /// is a small cache, not a log.
    pub fn set_active_config(&mut self, solution: &Path, config: &ConfigPlatform) -> Result<()> {
        let key = self.solution_key(solution);
        self.state.solutions.entry(key).or_default().active_config = Some(config.clone());
        write_state(&self.path, &self.state)
    }

    /// Fetch a solution's own last-used configuration/platform, if recorded. Reads the in-memory
    /// copy loaded at `open()`. Per ADR 20, `Store` is the only writer within a process so there is
    /// nothing on disk this copy could be stale against.
    pub fn active_config(&self, solution: &Path) -> Result<Option<ConfigPlatform>> {
        let key = self.solution_key(solution);
        Ok(self
            .state
            .solutions
            .get(&key)
            .and_then(|record| record.active_config.clone()))
    }

    /// True when nothing the solution depends on has changed since the last [`record_dependencies`]
    /// call. `false` for a solution with no recorded dependency set at all — nothing has been
    /// evaluated yet so there is nothing to compare against, and the safe default is "reparse".
    ///
    /// Only ever compares the already recorded set against the filesystem. It never re-evaluates to
    /// discover files that joined the dependency graph without touching a file already being tracked.
    pub fn is_up_to_date(&self, solution: &Path) -> Result<bool> {
        let key = self.solution_key(solution);
        let Some(record) = self.state.solutions.get(&key) else {
            return Ok(false);
        };

        for (path, recorded_mtime) in &record.files {
            match mtime_secs(Path::new(path)) {
                Ok(current) if current == *recorded_mtime => continue,
                // Missing file, deleted dependency and any other metadata failure count as stale,
                // not an error. Anything other than confirmed unchanged means reparse.
                _ => return Ok(false),
            }
        }

        Ok(true)
    }

    /// Record the current mtime of every file `solution`'s evaluation touched, replacing whatever
    /// was recorded previously. Called once per successful evaluation with the complete dependency
    /// set.
    ///
    /// Unlike `is_up_to_date`, a metadata failure here is an error.
    pub fn record_dependencies(&mut self, solution: &Path, files: &[PathBuf]) -> Result<()> {
        let mut recorded = BTreeMap::new();
        for file in files {
            let normalized = normalize(file);
            let mtime = mtime_secs(&normalized)?;
            recorded.insert(normalized.to_string_lossy().into_owned(), mtime);
        }

        let key = self.solution_key(solution);
        self.state.solutions.entry(key).or_default().files = recorded;
        write_state(&self.path, &self.state)
    }

    /// The key `solution` is stored under in `state.solutions`. This is the solution path relative
    /// to `self.root`, lowercased. Relative rather than absolute so relocating the whole workspace
    /// doesn't orphan every entry.
    fn solution_key(&self, solution: &Path) -> String {
        let root = normalize(&self.root);
        let solution = normalize(solution);
        relative_to(&root, &solution)
            .to_string_lossy()
            .to_lowercase()
    }
}

/// `path`'s current mtime, in whole seconds since the Unix epoch. Mod time before the epoch clamps
/// to 0.
fn mtime_secs(path: &Path) -> Result<i64> {
    let metadata = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    let modified = metadata.modified().map_err(|e| Error::io(path, e))?;
    let secs = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Ok(secs)
}

/// Serialize `state` to `path` atomically (temp file then rename).
fn write_state(path: &Path, state: &State) -> Result<()> {
    let json = serde_json::to_string_pretty(state).expect("State serializes without error");
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json.as_bytes()).map_err(|e| Error::io(&tmp_path, e))?;
    std::fs::rename(&tmp_path, path).map_err(|e| Error::io(path, e))?;
    Ok(())
}

/// Relative path from `base` to `target` (both already absolute and lexically normalized),
/// climbing with `..` when `target` isn't a descendant of `base`. Falls back to returning `target`
/// unchanged if the two share no common prefix. Component comparison is case-insensitive, matching
/// the reasoning `compdb`'s output-directory hash already applies: `solution_key` lowercases the
/// final joined string regardless, so components are pushed in their original case here.
///
/// Handrolled because this is the one place that needs it.
fn relative_to(base: &Path, target: &Path) -> PathBuf {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();

    let common = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| {
            a.as_os_str().to_string_lossy().to_lowercase()
                == b.as_os_str().to_string_lossy().to_lowercase()
        })
        .count();

    if common == 0 {
        return target.to_path_buf();
    }

    let mut result = PathBuf::new();
    for _ in common..base_components.len() {
        result.push("..");
    }
    for component in &target_components[common..] {
        result.push(component.as_os_str());
    }

    result
}
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::Error;
    use crate::testing::TempTree;

    /// A portable absolute path, independent of which OS the test runs on — `relative_to`'s
    /// own tests need to build and run on Linux CI even though the crate's only real runtime
    /// is Windows (CLAUDE.md §3). Mirrors `compdb.rs`'s own `absolute()` test helper.
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    // --- `Store::open`: shape, create-or-load, and error handling ---

    #[test]
    fn open_creates_a_fresh_state_file_with_the_expected_top_level_keys() {
        let tree = TempTree::new("open_creates_fresh_state");
        let sln2cc_dir = tree.path(".sln2cc");

        Store::open(&sln2cc_dir).expect("a missing state file is not an error");

        let contents = std::fs::read_to_string(sln2cc_dir.join("state.json"))
            .expect("open() must write the fresh state file to disk");
        let value: serde_json::Value = serde_json::from_str(&contents).expect("valid json");

        assert_eq!(value["version"], 1);
        assert_eq!(value["solutions"], serde_json::json!({}));
    }

    #[test]
    fn reopening_existing_state_round_trips_unchanged() {
        let tree = TempTree::new("reopen_round_trips");
        let sln2cc_dir = tree.path(".sln2cc");
        std::fs::create_dir_all(&sln2cc_dir).expect("temp tree is writable");
        let original = serde_json::json!({
            "version": 1,
            "solutions": {
                "app.sln": {
                    "known_configs": [{"configuration": "Debug", "platform": "x64"}],
                    "active_config": {"configuration": "Debug", "platform": "x64"},
                    "files": {"d:\\code\\app\\app.vcxproj": 1_700_000_000_i64}
                }
            }
        });
        std::fs::write(sln2cc_dir.join("state.json"), original.to_string())
            .expect("temp tree is writable");

        Store::open(&sln2cc_dir).expect("opens existing state");

        let reread: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(sln2cc_dir.join("state.json"))
                .expect("state file still exists"),
        )
        .expect("valid json");
        assert_eq!(
            reread, original,
            "open() must not rewrite already-valid state just by loading it"
        );
    }

    #[test]
    fn open_treats_a_missing_file_as_empty_state() {
        let tree = TempTree::new("missing_file_is_empty_state");
        let sln2cc_dir = tree.path(".sln2cc");

        let result = Store::open(&sln2cc_dir);

        assert!(
            result.is_ok(),
            "a missing state file must be treated as empty state, not an error"
        );
    }

    #[test]
    fn a_malformed_state_file_produces_a_clear_error_not_a_panic() {
        let tree = TempTree::new("malformed_state_file");
        let sln2cc_dir = tree.path(".sln2cc");
        std::fs::create_dir_all(&sln2cc_dir).expect("temp tree is writable");
        std::fs::write(sln2cc_dir.join("state.json"), "{ not valid json")
            .expect("temp tree is writable");

        let result = Store::open(&sln2cc_dir);

        assert!(matches!(result, Err(Error::StateParse { .. })));
    }

    /// Not in the issue's own test list, but the schema version field only earns its keep
    /// (per the issue's summary, "for future migrations") if an unsupported one is actually
    /// rejected — this is the only case that exercises it in MVP-18.
    #[test]
    fn an_unsupported_schema_version_is_rejected_clearly() {
        let tree = TempTree::new("unsupported_schema_version");
        let sln2cc_dir = tree.path(".sln2cc");
        std::fs::create_dir_all(&sln2cc_dir).expect("temp tree is writable");
        std::fs::write(
            sln2cc_dir.join("state.json"),
            serde_json::json!({"version": 999, "solutions": {}}).to_string(),
        )
        .expect("temp tree is writable");

        let result = Store::open(&sln2cc_dir);

        match result {
            Err(Error::StateVersion {
                found, supported, ..
            }) => {
                assert_eq!(found, 999);
                assert_eq!(supported, 1);
            }
            other => panic!("expected Error::StateVersion, got {other:?}"),
        }
    }

    // --- Solution-key computation: relative to the workspace root, not absolute ---
    //
    // Root-relative (rather than absolute) keys are what let the whole workspace move — a
    // clone to a new path, a drive letter change — without orphaning every entry. `--root`
    // is documented to diverge from where the solution actually lives ("CLI Surface" in
    // engineering_spec.md), so plain `Path::strip_prefix` isn't enough; `relative_to` climbs
    // with `..` for a solution beside or above root, and only falls back to an absolute path
    // when no relative path can be expressed at all (different Windows drives).

    #[test]
    fn relative_to_a_descendant_produces_no_climbs() {
        let root = absolute(&["Code", "App"]);
        let target = absolute(&["Code", "App", "Sub", "App.sln"]);

        let relative = relative_to(&root, &target);

        assert_eq!(relative, Path::new("Sub").join("App.sln"));
    }

    #[test]
    fn relative_to_a_sibling_climbs_with_dot_dot() {
        let root = absolute(&["Code", "App", "Workspace"]);
        let target = absolute(&["Code", "App", "Other", "App.sln"]);

        let relative = relative_to(&root, &target);

        assert_eq!(relative, Path::new("..").join("Other").join("App.sln"));
    }

    /// `relative_to` only needs case-insensitive matching for *detecting* the common prefix —
    /// it pushes `target`'s own components verbatim, so only `root`'s casing can vary here
    /// without changing the expected output. (The tail's casing is a `solution_key` guarantee,
    /// via the final `.to_lowercase()`; see `solution_key_is_stable_across_casing`.)
    #[test]
    fn relative_to_matches_the_common_prefix_regardless_of_its_casing() {
        let root = absolute(&["Code", "App"]);
        let differently_cased_root = absolute(&["CODE", "APP"]);
        let target = absolute(&["Code", "App", "Sub", "App.sln"]);

        assert_eq!(
            relative_to(&root, &target),
            relative_to(&differently_cased_root, &target)
        );
    }

    /// A different Windows drive letter has no relative-path representation at all, so
    /// `relative_to` must fall back to the target unchanged rather than producing a
    /// nonsensical climb. Windows-only: on Unix every absolute path shares a leading `/`, so
    /// this branch has no portable non-Windows trigger — matching the existing
    /// `if cfg!(windows)` conditionals elsewhere in the test suite (`paths.rs`, `compdb.rs`'s
    /// `absolute()` helper).
    #[cfg(windows)]
    #[test]
    fn relative_to_a_different_drive_falls_back_to_the_target_unchanged() {
        let root = PathBuf::from(r"C:\Code\App");
        let target = PathBuf::from(r"D:\Other\App.sln");

        assert_eq!(relative_to(&root, &target), target);
    }

    #[test]
    fn solution_key_is_relative_to_the_workspace_root_and_lowercased() {
        let tree = TempTree::new("solution_key_relative_to_root");
        let sln2cc_dir = tree.path(".sln2cc");
        let store = Store::open(&sln2cc_dir).expect("opens");
        let solution = tree.path("Sub/App.sln");

        let key = store.solution_key(&solution);

        let expected = Path::new("Sub")
            .join("App.sln")
            .to_string_lossy()
            .to_lowercase();
        assert_eq!(key, expected);
    }

    /// Windows paths are case-preserving but the filesystem is not — without this, the same
    /// solution invoked with different casing would silently split across two keys, the same
    /// hazard `compdb::output_dir_name_is_stable_across_casing` guards against for the
    /// hash-suffixed output directory. Unlike `relative_to`, `solution_key` lowercases its
    /// final result, so this is where casing stability end-to-end actually holds.
    #[test]
    fn solution_key_is_stable_across_casing() {
        let tree = TempTree::new("solution_key_stable_across_casing");
        let sln2cc_dir = tree.path(".sln2cc");
        let store = Store::open(&sln2cc_dir).expect("opens");

        let lower_key = store.solution_key(&tree.path("Sub/App.sln"));
        let upper_key = store.solution_key(&tree.root().join("SUB").join("APP.SLN"));

        assert_eq!(lower_key, upper_key);
    }

    // --- `set_active_config`/`active_config`: per-solution round trip ---

    fn debug_x64() -> ConfigPlatform {
        ConfigPlatform {
            configuration: "Debug".to_string(),
            platform: "x64".to_string(),
        }
    }

    fn release_win32() -> ConfigPlatform {
        ConfigPlatform {
            configuration: "Release".to_string(),
            platform: "Win32".to_string(),
        }
    }

    #[test]
    fn set_then_get_round_trips() {
        let tree = TempTree::new("set_then_get_round_trips");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        let solution = tree.path("App.sln");
        let config = debug_x64();

        store
            .set_active_config(&solution, &config)
            .expect("sets the active config");

        assert_eq!(store.active_config(&solution).expect("reads"), Some(config));
    }

    #[test]
    fn active_config_is_none_for_an_unknown_solution() {
        let tree = TempTree::new("active_config_unknown_solution");
        let store = Store::open(&tree.path(".sln2cc")).expect("opens");

        let result = store
            .active_config(&tree.path("Never.sln"))
            .expect("reading an unset solution is not an error");

        assert_eq!(
            result, None,
            "a solution with no recorded selection must read back None, not a default"
        );
    }

    #[test]
    fn set_active_config_overwrites_an_existing_selection() {
        let tree = TempTree::new("set_active_config_overwrites");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        let solution = tree.path("App.sln");

        store
            .set_active_config(&solution, &debug_x64())
            .expect("sets the first selection");
        store
            .set_active_config(&solution, &release_win32())
            .expect("sets the second selection");

        assert_eq!(
            store.active_config(&solution).expect("reads"),
            Some(release_win32()),
            "the most recent selection must win"
        );
    }

    #[test]
    fn two_solutions_hold_independent_selections() {
        let tree = TempTree::new("two_solutions_independent_selections");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        let first = tree.path("First.sln");
        let second = tree.path("Second.sln");

        store
            .set_active_config(&first, &debug_x64())
            .expect("sets the first solution's selection");
        store
            .set_active_config(&second, &release_win32())
            .expect("sets the second solution's selection");

        assert_eq!(
            store.active_config(&first).expect("reads"),
            Some(debug_x64()),
            "setting the second solution must not disturb the first"
        );
        assert_eq!(
            store.active_config(&second).expect("reads"),
            Some(release_win32()),
            "setting the first solution must not disturb the second"
        );
    }

    /// Not in the issue's own test list, but proves the write-through actually reaches
    /// `state.json` rather than only updating the in-memory copy — a second `Store::open`
    /// against the same `.sln2cc` directory has no way to see the selection otherwise.
    #[test]
    fn set_active_config_persists_to_disk() {
        let tree = TempTree::new("set_active_config_persists_to_disk");
        let sln2cc_dir = tree.path(".sln2cc");
        let solution = tree.path("App.sln");

        let mut store = Store::open(&sln2cc_dir).expect("opens the first handle");
        store
            .set_active_config(&solution, &debug_x64())
            .expect("sets the active config");

        let reopened = Store::open(&sln2cc_dir).expect("opens a second handle");

        assert_eq!(
            reopened.active_config(&solution).expect("reads"),
            Some(debug_x64()),
            "a fresh Store::open must observe a selection set by a prior handle"
        );
    }

    // --- `record_dependencies`/`is_up_to_date`: mtime-based staleness tracking ---
    //
    // `is_up_to_date` only ever compares the already-recorded file set against the filesystem —
    // it never re-evaluates to discover files that joined the dependency graph without touching a
    // file already being tracked. See ADR 22.

    use std::time::{Duration, SystemTime};

    #[test]
    fn is_up_to_date_is_false_for_a_solution_never_recorded() {
        let tree = TempTree::new("is_up_to_date_never_recorded");
        let store = Store::open(&tree.path(".sln2cc")).expect("opens");

        let result = store
            .is_up_to_date(&tree.path("App.sln"))
            .expect("checking an unrecorded solution is not an error");

        assert!(
            !result,
            "a solution with no recorded dependency set has nothing to compare against, so it \
             must report stale"
        );
    }

    #[test]
    fn is_up_to_date_is_true_when_nothing_recorded_has_changed() {
        let tree = TempTree::new("is_up_to_date_unchanged");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let project = tree.write("App.vcxproj", "project contents");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");

        store
            .record_dependencies(&solution, &[solution.clone(), project])
            .expect("records the dependency set");

        assert!(
            store
                .is_up_to_date(&solution)
                .expect("checking is not an error"),
            "nothing changed since record_dependencies, so this must report up to date"
        );
    }

    #[test]
    fn is_up_to_date_is_false_when_a_recorded_file_is_touched() {
        let tree = TempTree::new("is_up_to_date_touched");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let project = tree.write("App.vcxproj", "project contents");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        store
            .record_dependencies(&solution, &[solution.clone(), project])
            .expect("records the dependency set");

        tree.write("App.vcxproj", "changed project contents");
        tree.set_mtime("App.vcxproj", SystemTime::now() + Duration::from_secs(60));

        assert!(
            !store
                .is_up_to_date(&solution)
                .expect("checking is not an error"),
            "a recorded file whose mtime moved forward must report stale"
        );
    }

    #[test]
    fn is_up_to_date_is_false_when_a_recorded_file_is_deleted() {
        let tree = TempTree::new("is_up_to_date_deleted");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let project = tree.write("App.vcxproj", "project contents");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        store
            .record_dependencies(&solution, &[solution.clone(), project.clone()])
            .expect("records the dependency set");

        std::fs::remove_file(&project).expect("the temp tree file is removable");

        let result = store
            .is_up_to_date(&solution)
            .expect("a deleted dependency is stale, not an error");
        assert!(!result, "a deleted dependency must report stale");
    }

    /// The issue's own fourth case: "adding a new import invalidates the cache." Under this
    /// design that resolves to the same mechanism as touching a dependency, because adding an
    /// `<Import>` means editing the file that names it — here, `App.vcxproj` standing in for
    /// having gained a new `<Import>` line — and that file's own recorded mtime is what catches
    /// it. See ADR 22's discussion of why a full file-set diff isn't needed.
    #[test]
    fn is_up_to_date_is_false_after_a_new_import_is_added() {
        let tree = TempTree::new("is_up_to_date_new_import");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let project = tree.write("App.vcxproj", "project contents, no imports");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");
        store
            .record_dependencies(&solution, &[solution.clone(), project])
            .expect("records the dependency set from the first evaluation");

        tree.write("App.vcxproj", "project contents, now importing Extra.props");
        tree.set_mtime("App.vcxproj", SystemTime::now() + Duration::from_secs(60));

        assert!(
            !store
                .is_up_to_date(&solution)
                .expect("checking is not an error"),
            "editing the project file to add an import must invalidate the cache"
        );
    }

    #[test]
    fn record_dependencies_replaces_rather_than_merges_the_previous_set() {
        let tree = TempTree::new("record_dependencies_replaces");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let a = tree.write("A.props", "a contents");
        let b = tree.write("B.props", "b contents");
        let mut store = Store::open(&tree.path(".sln2cc")).expect("opens");

        store
            .record_dependencies(&solution, &[solution.clone(), a.clone(), b.clone()])
            .expect("records the first, larger dependency set");
        store
            .record_dependencies(&solution, &[solution.clone(), a])
            .expect("records the second, smaller dependency set");

        tree.write("B.props", "b contents changed");
        tree.set_mtime("B.props", SystemTime::now() + Duration::from_secs(60));

        assert!(
            store
                .is_up_to_date(&solution)
                .expect("checking is not an error"),
            "B.props dropped out of the second, replacing call, so changing it must not affect \
             staleness"
        );
    }

    /// Matches `set_active_config_persists_to_disk`'s existing precedent: proves the write-through
    /// reaches `state.json` rather than only the in-memory copy.
    #[test]
    fn record_dependencies_persists_to_disk() {
        let tree = TempTree::new("record_dependencies_persists_to_disk");
        let sln2cc_dir = tree.path(".sln2cc");
        let solution = tree.path("App.sln");
        tree.write("App.sln", "sln contents");
        let project = tree.write("App.vcxproj", "project contents");

        let mut store = Store::open(&sln2cc_dir).expect("opens the first handle");
        store
            .record_dependencies(&solution, &[solution.clone(), project])
            .expect("records the dependency set");

        let reopened = Store::open(&sln2cc_dir).expect("opens a second handle");

        assert!(
            reopened
                .is_up_to_date(&solution)
                .expect("checking is not an error"),
            "a fresh Store::open must observe a dependency set recorded by a prior handle"
        );
    }
}
