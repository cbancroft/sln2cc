//! MSBuild evaluation for `.vcxproj` files: the most complex part of the system.
//!
//! A `.vcxproj` is not self-contained — it imports `.props`/`.targets` files (including
//! from the Visual Studio installation), inherits properties through property sheets, and
//! gates values behind `Condition` attributes. Producing correct flags means evaluating
//! that graph for a specific configuration/platform.
//!
//! Target schema: Visual Studio 2019 and 2022. Older toolsets are out of scope for v1.

pub mod eval;
pub mod flags;
pub mod imports;
pub mod items;
pub mod paths;
pub mod project;
pub mod reserved;

pub use reserved::ProjectScope;

use std::path::{Path, PathBuf};

use crate::{ConfigPlatform, Reporter, Result, progress};

/// Fully evaluated compilation settings for a single source file.
#[derive(Debug, Clone)]
pub struct FileCompilation {
    pub file: PathBuf,
    pub include_paths: Vec<PathBuf>,
    pub defines: Vec<String>,
    /// Additional raw compiler flags, in MSVC (`cl`) form.
    pub flags: Vec<String>,
}

/// An evaluated project: every source file it owns, with resolved settings.
#[derive(Debug, Clone)]
pub struct EvaluatedProject {
    pub path: PathBuf,
    pub files: Vec<FileCompilation>,
}

/// Evaluate a `.vcxproj` for one configuration/platform, in MSBuild's three passes.
///
/// `scope.config` is the **project** pair rather than the solution's, per ADR 05. The caller maps one to the other with
/// [`crate::sln::SolutionProject::config_for`], and a `.vcxproj` never sees the solution's
/// spelling.
pub fn evaluate(scope: &ProjectScope, reporter: &mut Reporter) -> Result<EvaluatedProject> {
    // Pass 1: the import graph, over the bag `reserved` seeds.
    let tree = imports::resolve(&scope.project, reserved::seed(scope), reporter)?;

    // Warn if the project is not built in this configuration, as it will yield an empty
    // file/configuration list.
    let declared = items::declared_configurations(&tree, reporter)?;
    if !declared.is_empty() && !declared.contains(&scope.config) {
        let offered: Vec<String> = declared.iter().map(ConfigPlatform::to_string).collect();
        reporter.warn_at(
            &scope.project,
            None,
            format!(
                "the project does not declare `{}`; it declares {}. Settings for it may be missing.",
                scope.config,
                offered.join(", ")
            ),
        );
    }

    // Pass 2: Parse the deferred item groups against the properties set from Pass 1.
    let sources = items::settle(&tree, &scope.project, reporter)?;
    if sources.is_empty() {
        reporter.warn_at(
            &scope.project,
            None,
            "no `ClCompile` source files were found. \
             The project contributes nothing to the final database",
        );
    }
    progress!(
        reporter,
        "{}: {} source file(s)",
        scope.project.display(),
        sources.len()
    );

    // Pass 3 metadata into compilation flags
    let project_dir = scope
        .project
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let files = sources
        .iter()
        .map(|source| flags::compile(source, &project_dir, reporter))
        .collect();

    Ok(EvaluatedProject {
        path: scope.project.clone(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reporter;
    use crate::msbuild::reserved::ProjectScope;
    use crate::testing::TempTree;

    /// A `.vcxproj` shaped the way Visual Studio writes one, importing a shared property sheet.
    ///
    /// The import sits *between* the property group and the item definitions, which is where a real
    /// project puts its `PropertySheets` group — so the sheet's item definitions splice in ahead of
    /// the project's own and the project's compose on top of them.
    const PROJECT: &str = r#"  <ItemGroup Label="ProjectConfigurations">
    <ProjectConfiguration Include="Debug|x64">
      <Configuration>Debug</Configuration>
      <Platform>x64</Platform>
    </ProjectConfiguration>
    <ProjectConfiguration Include="Release|x64">
      <Configuration>Release</Configuration>
      <Platform>x64</Platform>
    </ProjectConfiguration>
  </ItemGroup>
  <PropertyGroup>
    <AppDefines>APP</AppDefines>
  </PropertyGroup>
  <Import Project="shared\common.props" />
  <ItemDefinitionGroup>
    <ClCompile>
      <AdditionalIncludeDirectories>$(ProjectDir)inc;%(AdditionalIncludeDirectories)</AdditionalIncludeDirectories>
      <PreprocessorDefinitions>$(AppDefines);%(PreprocessorDefinitions)</PreprocessorDefinitions>
      <LanguageStandard>stdcpp20</LanguageStandard>
      <Optimization>MaxSpeed</Optimization>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'">
    <ClCompile>
      <PreprocessorDefinitions>_DEBUG;%(PreprocessorDefinitions)</PreprocessorDefinitions>
      <RuntimeLibrary>MultiThreadedDebugDLL</RuntimeLibrary>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemGroup>
    <ClCompile Include="src\main.cpp" />
    <ClCompile Include="src\util.cpp">
      <PreprocessorDefinitions>UTIL;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
    <ClInclude Include="inc\app.h" />
  </ItemGroup>"#;

    /// The shared sheet, one directory below the project and reaching its own siblings.
    const SHEET: &str = r#"  <ItemDefinitionGroup>
    <ClCompile>
      <AdditionalIncludeDirectories>$(MSBuildThisFileDirectory)include;%(AdditionalIncludeDirectories)</AdditionalIncludeDirectories>
      <PreprocessorDefinitions>SHARED;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>"#;

    /// Write the project and its sheet, and hand back the scope to evaluate them under.
    ///
    /// The `.sln` is named but never written: seeding reads its path and nothing else, matching what
    /// `imports.rs`'s tests do for the same reason.
    fn app(dir: &TempTree, config: &str, platform: &str) -> ProjectScope {
        let project = dir.project("app/App.vcxproj", PROJECT);
        dir.project("app/shared/common.props", SHEET);

        ProjectScope {
            solution: dir.path("App.sln"),
            project,
            config: ConfigPlatform::new(config, platform),
        }
    }

    /// Evaluate with a quiet reporter, asserting nothing was warned about.
    fn evaluated(scope: &ProjectScope) -> EvaluatedProject {
        let mut reporter = Reporter::new(false);
        let project = evaluate(scope, &mut reporter).expect("evaluates");
        assert!(
            reporter.warnings().is_empty(),
            "unexpected warnings: {:?}",
            reporter.warnings()
        );
        project
    }

    fn evaluated_reporting(scope: &ProjectScope) -> (EvaluatedProject, Reporter) {
        let mut reporter = Reporter::new(false);
        let project = evaluate(scope, &mut reporter).expect("evaluates");
        (project, reporter)
    }

    /// The compiled file names, so assertions read the same on either platform.
    fn names(project: &EvaluatedProject) -> Vec<String> {
        project
            .files
            .iter()
            .map(|file| {
                file.file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn file<'a>(project: &'a EvaluatedProject, name: &str) -> &'a FileCompilation {
        project
            .files
            .iter()
            .find(|file| file.file.ends_with(name))
            .unwrap_or_else(|| panic!("{name} is not in the evaluated project"))
    }

    #[test]
    fn a_project_and_its_sheet_produce_one_compilation_per_source_file() {
        let dir = TempTree::new("evaluate");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        assert_eq!(project.path, scope.project);
        assert_eq!(
            names(&project),
            ["main.cpp", "util.cpp"],
            "the header is not a translation unit"
        );
    }

    #[test]
    fn include_directories_are_absolute_and_ordered_project_first() {
        // The project's own directories come before the sheet's because its `ItemDefinitionGroup`
        // prepends to the inherited list, which is the search order MSBuild would use.
        let dir = TempTree::new("evaluate-includes");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        assert_eq!(
            file(&project, "main.cpp").include_paths,
            [
                paths::normalize(&dir.path("app/inc")),
                paths::normalize(&dir.path("app/shared/include")),
            ]
        );
    }

    #[test]
    fn the_reserved_properties_are_seeded_before_the_walk() {
        // The wiring this issue owed MVP-11: `reserved::seed` had no caller, so `$(ProjectDir)`
        // expanded to nothing and every include path silently lost its prefix. An absolute path
        // under the project directory is the whole assertion.
        let dir = TempTree::new("evaluate-seeded");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        let first = &file(&project, "main.cpp").include_paths[0];
        assert!(
            first.is_absolute(),
            "`$(ProjectDir)inc` resolved to {first:?}, which is not absolute"
        );
        assert!(first.starts_with(paths::normalize(&dir.path("app"))));
    }

    #[test]
    fn a_property_sheet_keeps_its_own_directory_in_the_second_pass() {
        // `$(MSBuildThisFileDirectory)include` in a shared sheet has to mean the sheet's own
        // directory. The catch is that the sheet's `ItemDefinitionGroup` is *deferred* to pass 2,
        // and by then the walk has restored `MSBuildThisFile*` to the project — so settling against
        // the final bag as-is resolves it to the project directory and points clangd at a directory
        // that does not exist. ADR 11 calls this family load-bearing for exactly this case.
        let dir = TempTree::new("evaluate-thisfile");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        assert!(
            file(&project, "main.cpp")
                .include_paths
                .contains(&paths::normalize(&dir.path("app/shared/include"))),
            "the sheet's own directory was lost: {:?}",
            file(&project, "main.cpp").include_paths
        );
    }

    #[test]
    fn defines_compose_sheet_then_project_then_file() {
        // Three layers in one list, which is the composition the whole pass exists to get right:
        // the sheet contributes `SHARED`, the project `APP` and the Debug group `_DEBUG`, and
        // `util.cpp` prepends its own on top of all of them.
        let dir = TempTree::new("evaluate-defines");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        assert_eq!(
            file(&project, "main.cpp").defines,
            ["_DEBUG", "APP", "SHARED"]
        );
        assert_eq!(
            file(&project, "util.cpp").defines,
            ["UTIL", "_DEBUG", "APP", "SHARED"]
        );
    }

    #[test]
    fn only_the_selected_configurations_group_applies() {
        // Asking for `Release|x64` must not pick up the `Debug|x64` group. Getting this wrong yields
        // a database that generates cleanly and describes a different build.
        let dir = TempTree::new("evaluate-release");
        let scope = app(&dir, "Release", "x64");

        let project = evaluated(&scope);

        let defines = &file(&project, "main.cpp").defines;
        assert_eq!(defines, &["APP", "SHARED"]);
        assert!(
            !file(&project, "main.cpp")
                .flags
                .iter()
                .any(|flag| flag == "/MDd"),
            "the Debug runtime library must not reach a Release evaluation"
        );
    }

    #[test]
    fn flags_are_emitted_in_msvc_form_and_codegen_settings_are_skipped() {
        let dir = TempTree::new("evaluate-flags");
        let scope = app(&dir, "Debug", "x64");

        let project = evaluated(&scope);

        let flags = &file(&project, "main.cpp").flags;
        assert!(flags.contains(&"/std:c++20".to_string()), "{flags:?}");
        assert!(flags.contains(&"/MDd".to_string()), "{flags:?}");
        assert!(
            !flags.iter().any(|flag| flag.starts_with("/O")),
            "`Optimization` cannot change how a file parses: {flags:?}"
        );
        assert!(
            !flags.iter().any(|flag| flag.starts_with("/I")),
            "include directories belong in `include_paths`, not in `flags`: {flags:?}"
        );
        assert!(
            !flags.iter().any(|flag| flag.starts_with("/D")),
            "defines belong in `defines`, not in `flags`: {flags:?}"
        );
    }

    #[test]
    fn a_configuration_the_project_does_not_declare_warns() {
        // The pair evaluates perfectly cleanly, matches no configuration-gated group, and produces
        // source files with no flags — the plausible-but-wrong database CLAUDE.md §3 is about. The
        // solution can name a pair the project never declared, so this is reachable input.
        let dir = TempTree::new("evaluate-undeclared");
        let scope = app(&dir, "Debug", "ARM64");

        let (project, reporter) = evaluated_reporting(&scope);

        assert_eq!(
            names(&project),
            ["main.cpp", "util.cpp"],
            "warned about, not refused: MSBuild builds this too"
        );
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("Debug|ARM64")
                    && warning.severity == crate::Severity::Warning),
            "names the pair that is missing, as a warning: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn a_project_with_no_source_files_warns() {
        // A project whose sources are all behind a wildcard, or a header-only library. Empty is a
        // legitimate answer from `items::settle`, and this is where it gets said out loud.
        let dir = TempTree::new("evaluate-empty");
        let project_path = dir.project(
            "app/App.vcxproj",
            r#"  <ItemGroup>
    <ClInclude Include="inc\app.h" />
  </ItemGroup>"#,
        );
        let scope = ProjectScope {
            solution: dir.path("App.sln"),
            project: project_path,
            config: ConfigPlatform::new("Debug", "x64"),
        };

        let (project, reporter) = evaluated_reporting(&scope);

        assert!(project.files.is_empty());
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("ClCompile")
                    && warning.severity == crate::Severity::Warning),
            "says what was missing, as a warning: {:?}",
            reporter.warnings()
        );
    }

    /// The volume check MVP-13 is really about, asserted rather than eyeballed.
    ///
    /// `evaluated` already refuses any finding at all on the clean path, which is the strongest
    /// form of this. What that cannot say is that the *severity* split has not quietly
    /// reclassified an ordinary project's evaluation into a wall of warnings — so the same
    /// realistic project is run once more with the findings inspected directly.
    #[test]
    fn an_ordinary_project_produces_no_findings_at_either_level() {
        let dir = TempTree::new("evaluate-quiet");
        let scope = app(&dir, "Debug", "x64");

        let (_, reporter) = evaluated_reporting(&scope);

        assert!(
            reporter.warnings().is_empty(),
            "a project Visual Studio would write must evaluate in silence: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn a_missing_project_file_is_an_error_rather_than_an_empty_result() {
        let dir = TempTree::new("evaluate-missing");
        let scope = ProjectScope {
            solution: dir.path("App.sln"),
            project: dir.path("app/Gone.vcxproj"),
            config: ConfigPlatform::new("Debug", "x64"),
        };

        let error = evaluate(&scope, &mut Reporter::new(false)).expect_err("must not evaluate");

        assert_eq!(error.kind(), crate::ErrorKind::Io);
    }
}
