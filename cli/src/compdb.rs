//! `compile_commands.json` generation.
//!
//! Flags are emitted in MSVC (`cl`) form and consumed by clangd in clang-cl mode
//! (`--driver-mode=cl`) rather than being translated to native clang flags — translation
//! is the higher-fidelity-risk path.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::msbuild::{EvaluatedProject, FileCompilation};
use crate::toolchain::Toolchain;
use crate::{Error, Result};

/// Name of the file recording which solution directory a `.msvclsp/<name>-<hash>` directory
/// belongs to, so a hash collision between two different solutions can be detected instead of
/// silently overwriting output.
/// See "Storage Layout" in `engineering_spec.md`
const SOURCE_MARKER: &str = ".source";

/// FNV-1a over raw bytes. Hand rolled as the `DefaultHasher` algorithm is subject to change which
/// may lead to a reshuffle on future Rust upgrades. Will deprecate if Rust standardizes on an algorithm.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// The lowercased, normalized absolute directory a `.sln` lives in. Used for hash generation and
/// storing in the collision marker file.
///
/// Unicode aware.
fn normalized_source(sln_path: &Path) -> String {
    let dir = sln_path.parent().unwrap_or(Path::new(""));
    crate::msbuild::paths::normalize(dir)
        .to_string_lossy()
        .to_lowercase()
}

/// `<solution-stem>-<hash>`: The low 32 bits of the FNV-1a hash of [`normalized_source`] formatted
/// as 8 lowercase digits.
fn output_dir_name(sln_path: &Path) -> String {
    // A `.sln` reaching this point was already opened successfully during solution parsing so it
    // has a filename with an extension. This can't fail in normal operations.
    let stem = sln_path
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("a parsed .sln path always has a valid UTF-8 file stem");
    let hash = fnv1a(normalized_source(sln_path).as_bytes()) as u32;
    format!("{stem}-{hash:08x}")
}

/// Resolve the `.msvclsp/<solution-name>-<hash>` directory for `sln_path`, creating it (and
/// `root`, if needed) and writing its collision marker on first use.
///
/// A mismatched marker means a different solution's directory already claims this hash. The error
/// is reported as [`Error::StorageCollision`] rather than silently overwriting.
pub fn resolve_output_dir(root: &Path, sln_path: &Path) -> Result<PathBuf> {
    let output_dir = root.join(output_dir_name(sln_path));
    let marker_path = output_dir.join(SOURCE_MARKER);
    let source = normalized_source(sln_path);

    match std::fs::read_to_string(&marker_path) {
        Ok(existing) if existing == source => Ok(output_dir),
        Ok(existing) => Err(Error::StorageCollision {
            dir: output_dir,
            expected: source,
            found: existing,
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&output_dir).map_err(|e| Error::io(&output_dir, e))?;
            std::fs::write(&marker_path, &source).map_err(|e| Error::io(&marker_path, e))?;
            Ok(output_dir)
        }
        Err(e) => Err(Error::io(&marker_path, e)),
    }
}

/// A single entry in the compilation database.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    pub arguments: Vec<String>,
}

/// Build database entries from evaluated projects plus discovered toolchain include paths.
///
/// No sorting happens within this function. `EvaluatedProject`/`FileCompilation` are `Vec`-based so
/// iterating produces the deterministic output.
pub fn build(projects: &[EvaluatedProject], toolchain: &Toolchain) -> Result<Vec<CompileCommand>> {
    let mut commands = Vec::new();
    for project in projects {
        let directory = project.path.parent().unwrap_or(Path::new("")).to_path_buf();
        for project_file in &project.files {
            commands.push(CompileCommand {
                directory: directory.clone(),
                file: project_file.file.clone(),
                arguments: arguments(project_file, toolchain),
            });
        }
    }
    Ok(commands)
}

/// One project file's `arguments` vector: a clang-cl compatible invocation that clangd can parse directly.
///
/// Toolchain system includes are appended *after* the project includes, matching `cl`'s own search
/// order (project directories win a same-name-header tie). The file path trails everything else.
fn arguments(file: &FileCompilation, toolchain: &Toolchain) -> Vec<String> {
    let mut args = vec!["clang-cl".to_string(), "--driver-mode=cl".to_string()];

    args.extend(
        file.include_paths
            .iter()
            .map(|path| format!("/I{}", path.display())),
    );

    args.extend(
        toolchain
            .include_paths
            .iter()
            .map(|path| format!("/I{}", path.display())),
    );

    args.extend(file.defines.iter().map(|define| format!("/D{define}")));
    args.extend(file.flags.iter().cloned());
    args.push(file.file.display().to_string());

    args
}

/// One `compile_commands.json` entry. `PathBuf` is not serialized directly. Its `Serialize` impl
/// errors on non-UTF-8 paths, and `compdb::arguments` already commits to `.display()` for the same
/// fields, so this keeps one path-to-string convention instead of two.
#[derive(Serialize)]
struct Entry<'a> {
    directory: String,
    file: String,
    arguments: &'a [String],
}

impl<'a> From<&'a CompileCommand> for Entry<'a> {
    fn from(c: &'a CompileCommand) -> Self {
        Entry {
            directory: c.directory.display().to_string(),
            file: c.file.display().to_string(),
            arguments: &c.arguments,
        }
    }
}

/// Write the database to `<output_dir>/compile_commands.json`, atomically (temp file then rename)
/// so an interrupted run can never leave clangd reading a truncated DB.
pub fn write(commands: &[CompileCommand], output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir).map_err(|e| Error::io(output_dir, e))?;

    let entries: Vec<Entry> = commands.iter().map(Entry::from).collect();

    // `Entry`'s fields are only `String`/`Vec<String>` so this cannot fail for any value this crate
    // constructs
    let json =
        serde_json::to_string_pretty(&entries).expect("Entry serializes to JSON without error");

    let final_path = output_dir.join("compile_commands.json");
    let tmp_path = output_dir.join(".compile_commands.json.tmp");
    std::fs::write(&tmp_path, json.as_bytes()).map_err(|e| Error::io(&tmp_path, e))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| Error::io(&final_path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::testing::TempTree;

    /// A portable absolute path, independent of which OS the test runs on — the crate has to build
    /// and test on Linux CI even though its only real runtime is Windows (CLAUDE.md §3).
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    fn toolchain(include_paths: &[PathBuf]) -> Toolchain {
        Toolchain {
            install_path: absolute(&["vs"]),
            vc_targets_path: absolute(&["vs", "MSBuild", "Microsoft", "VC", "v170"]),
            include_paths: include_paths.to_vec(),
            toolset_version: "14.38.33130".to_string(),
            sdk_version: "10.0.22621.0".to_string(),
        }
    }

    fn project(vcxproj: PathBuf, files: Vec<FileCompilation>) -> EvaluatedProject {
        EvaluatedProject {
            path: vcxproj,
            files,
        }
    }

    fn file(
        name: &str,
        include_paths: &[PathBuf],
        defines: &[&str],
        flags: &[&str],
    ) -> FileCompilation {
        FileCompilation {
            file: absolute(&["code", "app", "src", name]),
            include_paths: include_paths.to_vec(),
            defines: defines.iter().map(|d| d.to_string()).collect(),
            flags: flags.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn entries_carry_absolute_directory_and_file() {
        let vcxproj = absolute(&["code", "app", "App.vcxproj"]);
        let projects = [project(vcxproj, vec![file("main.cpp", &[], &[], &[])])];

        let commands = build(&projects, &toolchain(&[])).expect("builds");

        assert_eq!(commands.len(), 1);
        assert!(commands[0].directory.is_absolute());
        assert!(commands[0].file.is_absolute());
        assert_eq!(commands[0].directory, absolute(&["code", "app"]));
    }

    #[test]
    fn project_includes_precede_toolchain_includes() {
        // Toolchain system includes must land after project includes, matching `cl`'s own search
        // order (a project header wins a same-name tie against a system one).
        let project_include = absolute(&["code", "app", "inc"]);
        let toolchain_include = absolute(&["vs", "VC", "include"]);
        let projects = [project(
            absolute(&["code", "app", "App.vcxproj"]),
            vec![file(
                "main.cpp",
                std::slice::from_ref(&project_include),
                &[],
                &[],
            )],
        )];

        let commands = build(
            &projects,
            &toolchain(std::slice::from_ref(&toolchain_include)),
        )
        .expect("builds");

        let includes: Vec<&str> = commands[0]
            .arguments
            .iter()
            .filter(|arg| arg.starts_with("/I"))
            .map(|arg| arg.as_str())
            .collect();
        assert_eq!(
            includes,
            [
                format!("/I{}", project_include.display()),
                format!("/I{}", toolchain_include.display()),
            ]
        );
    }

    #[test]
    fn arguments_begin_with_a_clang_cl_driver_invocation() {
        let projects = [project(
            absolute(&["code", "app", "App.vcxproj"]),
            vec![file("main.cpp", &[], &[], &[])],
        )];

        let commands = build(&projects, &toolchain(&[])).expect("builds");

        assert_eq!(
            &commands[0].arguments[..2],
            ["clang-cl", "--driver-mode=cl"]
        );
    }

    #[test]
    fn the_file_is_the_trailing_argument_after_defines_and_flags() {
        let projects = [project(
            absolute(&["code", "app", "App.vcxproj"]),
            vec![file("main.cpp", &[], &["APP"], &["/std:c++20", "/MDd"])],
        )];

        let commands = build(&projects, &toolchain(&[])).expect("builds");
        let arguments = &commands[0].arguments;

        assert_eq!(
            arguments.last().unwrap(),
            &commands[0].file.display().to_string()
        );
        assert!(arguments.contains(&"/DAPP".to_string()));
        // `flags` keeps its own settled order (`AdditionalOptions` last per ADR 12) — not resorted.
        let std_pos = arguments.iter().position(|a| a == "/std:c++20").unwrap();
        let mdd_pos = arguments.iter().position(|a| a == "/MDd").unwrap();
        assert!(std_pos < mdd_pos);
        assert!(mdd_pos < arguments.len() - 1);
    }

    #[test]
    fn build_is_deterministic_across_runs() {
        let projects = [project(
            absolute(&["code", "app", "App.vcxproj"]),
            vec![
                file(
                    "main.cpp",
                    &[absolute(&["code", "app", "inc"])],
                    &["APP"],
                    &["/MDd"],
                ),
                file("util.cpp", &[], &["UTIL"], &[]),
            ],
        )];
        let tc = toolchain(&[absolute(&["vs", "VC", "include"])]);

        let first = build(&projects, &tc).expect("builds");
        let second = build(&projects, &tc).expect("builds");

        assert_eq!(first, second);
    }

    #[test]
    fn the_same_file_in_two_projects_produces_two_entries() {
        // MVP-12 dedups only within one project; a file two projects both compile legitimately
        // yields two entries here, each keeping its own project's flags.
        let shared = absolute(&["code", "shared", "util.cpp"]);
        let mut file_a = file("util.cpp", &[], &["A"], &[]);
        file_a.file = shared.clone();
        let mut file_b = file("util.cpp", &[], &["B"], &[]);
        file_b.file = shared.clone();

        let projects = [
            project(absolute(&["code", "a", "A.vcxproj"]), vec![file_a]),
            project(absolute(&["code", "b", "B.vcxproj"]), vec![file_b]),
        ];

        let commands = build(&projects, &toolchain(&[])).expect("builds");

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].file, shared);
        assert_eq!(commands[1].file, shared);
        assert!(commands[0].arguments.contains(&"/DA".to_string()));
        assert!(commands[1].arguments.contains(&"/DB".to_string()));
    }

    #[test]
    fn a_project_with_no_files_contributes_no_entries() {
        let projects = [project(absolute(&["code", "app", "App.vcxproj"]), vec![])];

        let commands = build(&projects, &toolchain(&[])).expect("builds");

        assert!(commands.is_empty());
    }

    // --- MVP-17: storage directory naming and collision detection ---

    /// A directory hashed under two different-case spellings must still land in the same
    /// `.msvclsp` subdirectory — Windows paths are case-preserving but the filesystem is
    /// case-insensitive, so hashing as-written would silently split one solution's artifacts
    /// across two directories depending on invocation casing.
    #[test]
    fn output_dir_name_is_stable_across_casing() {
        let lower = absolute(&["code", "app", "App.sln"]);
        // Uppercase only the directory components — the filename stays `App.sln` verbatim,
        // since `output_dir_name` hashes the solution's *directory*, not the file name, and
        // the stem it reads back out is taken from the untouched original path.
        let dir = lower.parent().expect("path has a parent");
        let mut upper = PathBuf::new();
        for component in dir.iter() {
            upper.push(component.to_string_lossy().to_uppercase());
        }
        upper.push("App.sln");

        assert_eq!(output_dir_name(&lower), output_dir_name(&upper));
    }

    /// Two solutions that happen to share a `.sln` filename in different directories must not
    /// collide on their `.msvclsp` output directory name.
    #[test]
    fn output_dir_name_differs_for_different_source_directories() {
        let a = absolute(&["code", "a", "App.sln"]);
        let b = absolute(&["code", "b", "App.sln"]);

        assert_ne!(output_dir_name(&a), output_dir_name(&b));
    }

    #[test]
    fn resolve_output_dir_creates_the_directory_and_marker() {
        let tree = TempTree::new("resolve_output_dir_creates");
        // `.msvclsp` itself does not exist yet, proving `create_dir_all` reaches all the way up.
        let msvclsp_root = tree.path(".msvclsp");
        let sln = tree.path("code/app/App.sln");

        let output_dir = resolve_output_dir(&msvclsp_root, &sln).expect("resolves");

        assert!(output_dir.starts_with(&msvclsp_root));
        let marker = std::fs::read_to_string(output_dir.join(".source")).expect("marker exists");
        assert_eq!(marker, normalized_source(&sln));
    }

    #[test]
    fn resolve_output_dir_is_idempotent_for_the_same_solution() {
        let tree = TempTree::new("resolve_output_dir_idempotent");
        let msvclsp_root = tree.path(".msvclsp");
        let sln = tree.path("code/app/App.sln");

        let first = resolve_output_dir(&msvclsp_root, &sln).expect("resolves");
        let second = resolve_output_dir(&msvclsp_root, &sln).expect("resolves again");

        assert_eq!(first, second);
    }

    /// A hash collision between two different solution directories must fail loudly rather than
    /// silently share or overwrite the existing directory (CLAUDE.md §3, "never silently
    /// guess"). A real FNV-1a collision can't be constructed by hand, so this seeds the marker
    /// directly at the path `output_dir_name` would compute for `sln`, with a different
    /// recorded source, simulating one.
    #[test]
    fn resolve_output_dir_rejects_a_genuine_collision() {
        let tree = TempTree::new("resolve_output_dir_collision");
        let msvclsp_root = tree.path(".msvclsp");
        let sln = tree.path("code/app/App.sln");
        let output_dir = msvclsp_root.join(output_dir_name(&sln));
        std::fs::create_dir_all(&output_dir).expect("temp tree is writable");
        std::fs::write(output_dir.join(".source"), "d:\\code\\someone-else")
            .expect("temp tree is writable");

        let result = resolve_output_dir(&msvclsp_root, &sln);

        assert!(matches!(result, Err(Error::StorageCollision { .. })));
        assert_eq!(
            std::fs::read_to_string(output_dir.join(".source")).expect("marker exists"),
            "d:\\code\\someone-else",
            "a rejected collision must not overwrite the existing marker"
        );
    }

    #[test]
    fn write_round_trips_entries_through_json() {
        let tree = TempTree::new("write_round_trips");
        let output_dir = tree.path("out");
        let commands = vec![CompileCommand {
            directory: absolute(&["code", "app"]),
            file: absolute(&["code", "app", "main.cpp"]),
            arguments: vec!["clang-cl".to_string(), "main.cpp".to_string()],
        }];

        write(&commands, &output_dir).expect("writes");

        let contents =
            std::fs::read_to_string(output_dir.join("compile_commands.json")).expect("file exists");
        let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
        assert_eq!(
            parsed[0]["directory"],
            commands[0].directory.display().to_string()
        );
        assert_eq!(parsed[0]["file"], commands[0].file.display().to_string());
        assert_eq!(
            parsed[0]["arguments"],
            serde_json::json!(["clang-cl", "main.cpp"])
        );
    }

    #[test]
    fn write_replaces_an_existing_file_atomically() {
        let tree = TempTree::new("write_replaces");
        let output_dir = tree.path("out");
        let first = vec![CompileCommand {
            directory: absolute(&["code", "app"]),
            file: absolute(&["code", "app", "old.cpp"]),
            arguments: vec!["clang-cl".to_string()],
        }];
        let second = vec![CompileCommand {
            directory: absolute(&["code", "app"]),
            file: absolute(&["code", "app", "new.cpp"]),
            arguments: vec!["clang-cl".to_string()],
        }];

        write(&first, &output_dir).expect("writes");
        write(&second, &output_dir).expect("writes again");

        let contents =
            std::fs::read_to_string(output_dir.join("compile_commands.json")).expect("file exists");
        assert!(contents.contains("new.cpp"));
        assert!(!contents.contains("old.cpp"));
        assert!(
            !output_dir.join(".compile_commands.json.tmp").exists(),
            "the temporary file must not survive a completed write"
        );
    }

    /// `write` does no reordering of its own — `Entry` field order is fixed at compile time and
    /// `build`'s own determinism is already covered by `build_is_deterministic_across_runs` — but
    /// the issue's own test list asks for this at the serialized-output level too, so this checks
    /// the on-disk bytes rather than the `Vec<CompileCommand>` that produced them.
    #[test]
    fn write_produces_identical_bytes_across_runs() {
        let tree = TempTree::new("write_stable_ordering");
        let output_dir = tree.path("out");
        let commands = vec![
            CompileCommand {
                directory: absolute(&["code", "app"]),
                file: absolute(&["code", "app", "a.cpp"]),
                arguments: vec!["clang-cl".to_string(), "/DA".to_string()],
            },
            CompileCommand {
                directory: absolute(&["code", "app"]),
                file: absolute(&["code", "app", "b.cpp"]),
                arguments: vec!["clang-cl".to_string(), "/DB".to_string()],
            },
        ];

        write(&commands, &output_dir).expect("writes");
        let first =
            std::fs::read_to_string(output_dir.join("compile_commands.json")).expect("file exists");
        write(&commands, &output_dir).expect("writes again");
        let second =
            std::fs::read_to_string(output_dir.join("compile_commands.json")).expect("file exists");

        assert_eq!(first, second);
    }
}
