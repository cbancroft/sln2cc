//! `compile_commands.json` generation.
//!
//! Flags are emitted in MSVC (`cl`) form and consumed by clangd in clang-cl mode
//! (`--driver-mode=cl`) rather than being translated to native clang flags — translation
//! is the higher-fidelity-risk path.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::msbuild::{EvaluatedProject, FileCompilation};
use crate::toolchain::Toolchain;

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

/// Write the database to `.msvclsp/<solution-name>/compile_commands.json`.
pub fn write(_commands: &[CompileCommand], _out: &Path) -> Result<()> {
    todo!("serialize compile_commands.json")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
