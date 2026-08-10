//! The properties MSBuild supplies that no project file defines.
//!
//! Files reference `$(SolutionDir)`, `$(ProjectDir)` and `$(MSBuildThisFileDirectory)` as though
//! they were always there, because under MSBuild they are: some are *reserved* and computed from
//! the file being evaluated, the rest are *global*, passed in by the solution build. Evaluating
//! against an empty bag expands every one of them to nothing and yields include paths that have
//! quietly lost their prefix — which is the failure CLAUDE.md §3 is about, since clangd will report
//! a missing header and name no cause.
//!
//! This module owns the *policy*: which names exist and what they hold. `eval.rs` owns the
//! *mechanism* that keeps a project file from overwriting them, and `imports.rs` deliberately
//! learns neither. See [ADR 11](../../../docs/adr/11-reserved-property-seeding.md).
//!
//! Toolchain-derived properties — `$(VCTargetsPath)`, `$(VSInstallDir)`,
//! `$(MSBuildExtensionsPath)` — are **not** here. They need a discovered Visual Studio
//! installation, which is MVP-15's to supply, and until it does the imports that depend on them
//! resolve to an empty path and are skipped with a warning.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use crate::ConfigPlatform;
use crate::msbuild::eval::Properties;
use crate::toolchain::Toolchain;

/// The solution, project and configuration one evaluation runs for
///
/// Uses Path rather than [`crate::sln::Solution`] so unit-testing can be run without building
/// a full fledged Solution.
#[derive(Clone, Debug)]
pub struct ProjectScope {
    /// Absolute path to the `.sln`.
    pub solution: PathBuf,
    /// Absolute path to the `.vcxproj` being evaluated.
    pub project: PathBuf,
    /// The project-level configuration/platform pair.
    pub config: ConfigPlatform,
    /// `None` until a subcommand wires up `ToolchainProvider::discover`. While `None`, associated
    /// properties remain unseeded.
    pub toolchain: Option<Toolchain>,
}

/// Fills out a Properties object with everything derivable from `scope`, ready to be handed to
/// `imports::resolve` or any other resolver.
pub fn seed(scope: &ProjectScope) -> Properties {
    let mut props = Properties::new();

    props.set_reserved("SolutionPath", text(&scope.solution));
    props.set_reserved("SolutionDir", directory(&scope.solution));
    props.set_reserved("SolutionFileName", file_name(&scope.solution));
    props.set_reserved("SolutionName", stem(&scope.solution));
    props.set_reserved("SolutionExt", extension(&scope.solution));

    props.set_reserved("Configuration", scope.config.configuration.clone());
    props.set_reserved("Platform", scope.config.platform.clone());

    // MSBuilds own spelling of project info, and the only place where a directory has no trailing slash
    props.set_reserved("MSBuildProjectFullPath", text(&scope.project));
    props.set_reserved("MSBuildProjectDirectory", parent(&scope.project));
    props.set_reserved(
        "MSBuildProjectDirectoryNoRoot",
        strip_root(&parent(&scope.project)),
    );
    props.set_reserved("MSBuildProjectFile", file_name(&scope.project));
    props.set_reserved("MSBuildProjectName", stem(&scope.project));
    props.set_reserved("MSBuildProjectExtension", extension(&scope.project));

    // Set so the root file always has a `restore` point.
    set_this_file(&mut props, &scope.project);

    // Not reserved, but derived from the project anyway
    props.set("ProjectPath", text(&scope.project));
    props.set("ProjectDir", directory(&scope.project));
    props.set("ProjectFileName", file_name(&scope.project));
    props.set("ProjectName", stem(&scope.project));
    props.set("ProjectExt", extension(&scope.project));

    if let Some(toolchain) = &scope.toolchain {
        props.set_reserved(
            "VSInstallDir",
            with_trailing_separator(&toolchain.install_path),
        );
        props.set_reserved(
            "MSBuildExtensionsPath",
            with_trailing_separator(&toolchain.install_path.join("MSBuild")),
        );
        props.set_reserved(
            "VCTargetsPath",
            with_trailing_separator(&toolchain.vc_targets_path),
        );
    }
    props
}
/// `path` with a trailing separator. Unlike `directory()`, which is written for a file path, this
/// is for a path that already names a directory, such as `Toolchain::install_path` and
/// `Toolchain::vc_targets_path`
fn with_trailing_separator(path: &Path) -> String {
    format!("{}{MAIN_SEPARATOR}", text(path))
}

/// The `MSBuildThisFile*` values displaced by [`enter_file`]. Opaque. Hand it back to [`restore`].
#[derive(Debug, Clone)]
#[must_use = "the displaced file scope must be restored when an import returns"]
pub struct FileScope(PathBuf);

/// Point `MSBuildThisFile*` at a particular `file` for the duration of that files evaluation.
pub fn enter_file(props: &mut Properties, file: &Path) -> FileScope {
    let displaced = PathBuf::from(props.get("MSBuildThisFileFullPath").unwrap_or_default());
    set_this_file(props, file);
    FileScope(displaced)
}

/// Restore what [`enter_file`] displaced.
pub fn restore(props: &mut Properties, scope: FileScope) {
    set_this_file(props, &scope.0);
}

/// Set the `MSBuildThis*` values from the given file.
/// `MSBuildThisFileDirectory` has a trailing separator, `MSBuildProjectDirectory` does not. This is
/// a quirk of MSBuild.
fn set_this_file(props: &mut Properties, file: &Path) {
    props.set_reserved("MSBuildThisFileFullPath", text(file));
    props.set_reserved("MSBuildThisFileDirectory", directory(file));
    props.set_reserved(
        "MSBuildThisFileDirectoryNoRoot",
        strip_root(&directory(file)),
    );
    props.set_reserved("MSBuildThisFile", file_name(file));
    props.set_reserved("MSBuildThisFileName", stem(file));
    props.set_reserved("MSBuildThisFileExtension", extension(file));
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `path`'s directory without a trailing separator.
fn parent(path: &Path) -> String {
    path.parent().map(text).unwrap_or_default()
}

/// `path`'s directory with a trailing separator
fn directory(path: &Path) -> String {
    match path.parent() {
        Some(parent) => format!("{}{MAIN_SEPARATOR}", parent.to_string_lossy()),
        None => String::new(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The extension **including** the dot.
fn extension(path: &Path) -> String {
    match path.extension() {
        Some(extension) => format!(".{}", extension.to_string_lossy()),
        None => String::new(),
    }
}

/// `directory` without its drive or leading separator, preserving any trailing one.
///
/// `D:\Code\app` becomes `code\app`. Strips by component rather than trimming prefixes, so it works
/// on UNC paths too.
fn strip_root(directory: &str) -> String {
    let path = Path::new(directory);
    let mut out = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            other => out.push(other),
        }
    }

    let stripped = text(&out);
    if directory.ends_with(MAIN_SEPARATOR) && !stripped.is_empty() {
        return format!("{stripped}{MAIN_SEPARATOR}");
    }

    stripped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigPlatform;
    use crate::msbuild::eval::Properties;
    use crate::toolchain::Toolchain;
    use std::path::{Path, PathBuf};

    /// Spelled once, so every expectation below is built the way the host platform builds a path.
    const SEP: char = std::path::MAIN_SEPARATOR;

    /// An absolute path built from `parts`, spelled the way the host platform spells one.
    ///
    /// Seeded values come out of `PathBuf`, so a hard-coded `D:\code\app` would assert nothing on
    /// the Linux CI runner — and the drive prefix is exactly what the `NoRoot` variants have to
    /// strip, so the two platforms give that code different work to do.
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    /// `parts` joined with the host separator and no leading one — how a `NoRoot` value reads.
    fn relative(parts: &[&str]) -> String {
        parts.join(&SEP.to_string())
    }

    /// `path` with a trailing separator — how a directory-valued property reads.
    fn directory_of(path: &Path) -> String {
        format!("{}{SEP}", path.display())
    }

    fn text(path: &Path) -> String {
        path.display().to_string()
    }

    /// The solution and project every case below is seeded from.
    ///
    /// The project deliberately sits one directory *below* the solution, since that is the ordinary
    /// layout and it is what makes `$(SolutionDir)` and `$(ProjectDir)` distinguishable.
    fn scope() -> ProjectScope {
        ProjectScope {
            solution: absolute(&["code", "app", "App.sln"]),
            project: absolute(&["code", "app", "lib", "Lib.vcxproj"]),
            config: ConfigPlatform::new("Debug", "x64"),
            toolchain: None,
        }
    }

    /// A fake but internally consistent `Toolchain`, standing in for a real `discover()` result.
    fn toolchain() -> Toolchain {
        Toolchain {
            install_path: absolute(&["vs", "2022", "community"]),
            vc_targets_path: absolute(&[
                "vs",
                "2022",
                "community",
                "MSBuild",
                "Microsoft",
                "VC",
                "v170",
            ]),
            include_paths: Vec::new(),
            toolset_version: "14.38.33130".to_string(),
            sdk_version: "10.0.22621.0".to_string(),
        }
    }

    /// Assert `name` holds `expected`, naming the property when it does not.
    fn assert_property(props: &Properties, name: &str, expected: &str) {
        assert_eq!(props.get(name), Some(expected), "$({name})");
    }

    #[test]
    fn the_solution_family_comes_from_the_sln_path() {
        let props = seed(&scope());

        assert_property(
            &props,
            "SolutionPath",
            &text(&absolute(&["code", "app", "App.sln"])),
        );
        assert_property(
            &props,
            "SolutionDir",
            &directory_of(&absolute(&["code", "app"])),
        );
        assert_property(&props, "SolutionFileName", "App.sln");
        assert_property(&props, "SolutionName", "App");
        assert_property(&props, "SolutionExt", ".sln");
    }

    #[test]
    fn the_configuration_pair_is_the_one_the_scope_carries() {
        // Per ADR 05 this is the *project* pair, not the solution-level pair the user picked: a
        // solution's `Debug|x64` legitimately means `Debug|Win32` for one of its members, and every
        // `Condition="'$(Configuration)|$(Platform)'=='...'"` in that project is written against the
        // project's own spelling. Choosing between them is the caller's job, not this module's.
        let props = seed(&ProjectScope {
            config: ConfigPlatform::new("Release", "Win32"),
            ..scope()
        });

        assert_property(&props, "Configuration", "Release");
        assert_property(&props, "Platform", "Win32");
    }

    #[test]
    fn the_msbuild_project_family_comes_from_the_vcxproj_path() {
        let props = seed(&scope());

        assert_property(
            &props,
            "MSBuildProjectFullPath",
            &text(&absolute(&["code", "app", "lib", "Lib.vcxproj"])),
        );
        assert_property(
            &props,
            "MSBuildProjectDirectory",
            &text(&absolute(&["code", "app", "lib"])),
        );
        assert_property(&props, "MSBuildProjectFile", "Lib.vcxproj");
        assert_property(&props, "MSBuildProjectName", "Lib");
        assert_property(&props, "MSBuildProjectExtension", ".vcxproj");
    }

    #[test]
    fn the_project_family_is_the_same_paths_in_visual_studios_spelling() {
        // `Project*` and `MSBuildProject*` describe the same file. They exist separately because
        // MSBuild computes the second and `Microsoft.Common.props` derives the first from it.
        let props = seed(&scope());

        assert_property(
            &props,
            "ProjectPath",
            &text(&absolute(&["code", "app", "lib", "Lib.vcxproj"])),
        );
        assert_property(
            &props,
            "ProjectDir",
            &directory_of(&absolute(&["code", "app", "lib"])),
        );
        assert_property(&props, "ProjectFileName", "Lib.vcxproj");
        assert_property(&props, "ProjectName", "Lib");
        assert_property(&props, "ProjectExt", ".vcxproj");
    }

    #[test]
    fn a_directory_property_carries_a_trailing_separator_except_msbuildprojectdirectory() {
        // MSBuild's own asymmetry, and the one detail here that is worth more than the rest put
        // together: project files concatenate `$(ProjectDir)inc` directly, so a missing separator
        // yields `...\libinc` — a path that does not exist, from a database that looks right. It is
        // also why `Microsoft.Common.props` writes `<ProjectDir>$(MSBuildProjectDirectory)\</...>`.
        let props = seed(&scope());

        for name in [
            "SolutionDir",
            "ProjectDir",
            "MSBuildThisFileDirectory",
            "MSBuildThisFileDirectoryNoRoot",
        ] {
            let value = props.get(name).unwrap_or_default();
            assert!(
                value.ends_with(SEP),
                "$({name}) must end in a separator: {value:?}"
            );
        }

        for name in ["MSBuildProjectDirectory", "MSBuildProjectDirectoryNoRoot"] {
            let value = props.get(name).unwrap_or_default();
            assert!(
                !value.ends_with(SEP),
                "$({name}) must not end in a separator: {value:?}"
            );
        }
    }

    #[test]
    fn the_noroot_variants_drop_the_drive_or_leading_separator() {
        let props = seed(&scope());

        assert_property(
            &props,
            "MSBuildProjectDirectoryNoRoot",
            &relative(&["code", "app", "lib"]),
        );
        assert_property(
            &props,
            "MSBuildThisFileDirectoryNoRoot",
            &format!("{}{SEP}", relative(&["code", "app", "lib"])),
        );
    }

    #[test]
    #[cfg(windows)]
    fn a_unc_path_loses_its_whole_prefix_not_two_characters() {
        // Why the root is stripped component-wise rather than by trimming: `\\server\share` is one
        // `Component::Prefix`, so trimming a fixed number of characters would leave `server\share`
        // glued to the front of every `NoRoot` value. A shared property sheet on a build server is
        // an ordinary place for a project to live.
        let props = seed(&ProjectScope {
            solution: PathBuf::from(r"\\server\share\code\App.sln"),
            project: PathBuf::from(r"\\server\share\code\lib\Lib.vcxproj"),
            config: ConfigPlatform::new("Debug", "x64"),
            toolchain: None,
        });

        assert_property(&props, "MSBuildProjectDirectoryNoRoot", r"code\lib");
    }

    #[test]
    fn every_seeded_property_holds_something() {
        // A misspelled name in `seed` is invisible: the property it was meant to define stays
        // undefined, expands to nothing, and the include path it fed silently loses a directory.
        // This list is the contract, so adding a property means adding it here too.
        const SEEDED: &[&str] = &[
            "SolutionPath",
            "SolutionDir",
            "SolutionFileName",
            "SolutionName",
            "SolutionExt",
            "Configuration",
            "Platform",
            "MSBuildProjectFullPath",
            "MSBuildProjectDirectory",
            "MSBuildProjectDirectoryNoRoot",
            "MSBuildProjectFile",
            "MSBuildProjectName",
            "MSBuildProjectExtension",
            "MSBuildThisFileFullPath",
            "MSBuildThisFileDirectory",
            "MSBuildThisFileDirectoryNoRoot",
            "MSBuildThisFile",
            "MSBuildThisFileName",
            "MSBuildThisFileExtension",
            "ProjectPath",
            "ProjectDir",
            "ProjectFileName",
            "ProjectName",
            "ProjectExt",
        ];

        let props = seed(&scope());

        for name in SEEDED {
            let value = props.get(name);
            assert!(value.is_some(), "$({name}) was never seeded");
            assert_ne!(value, Some(""), "$({name}) was seeded empty");
        }
    }

    // -------------------------------------------------------------------------------------------
    // MVP-15: the toolchain family, extending ADR 11's reserved space
    // -------------------------------------------------------------------------------------------

    #[test]
    fn toolchain_properties_are_seeded_when_a_toolchain_is_present() {
        let toolchain = toolchain();
        let props = seed(&ProjectScope {
            toolchain: Some(toolchain.clone()),
            ..scope()
        });

        assert_property(
            &props,
            "VSInstallDir",
            &directory_of(&toolchain.install_path),
        );
        assert_property(
            &props,
            "MSBuildExtensionsPath",
            &directory_of(&toolchain.install_path.join("MSBuild")),
        );
        assert_property(
            &props,
            "VCTargetsPath",
            &directory_of(&toolchain.vc_targets_path),
        );
    }

    #[test]
    fn toolchain_properties_stay_undefined_without_a_toolchain() {
        // Matches the "empty path, warned about" behavior `imports.rs` relies on today: seeding
        // these empty rather than leaving them absent would read as *defined* per ADR 11's
        // `Condition="'$(X)' != ''"` distinction and silently change what gets warned about.
        let props = seed(&scope());

        for name in ["VCTargetsPath", "VSInstallDir", "MSBuildExtensionsPath"] {
            assert_eq!(
                props.get(name),
                None,
                "$({name}) must stay undefined without a toolchain"
            );
        }
    }

    #[test]
    fn every_toolchain_seeded_property_holds_something() {
        // The toolchain-family counterpart to `every_seeded_property_holds_something`: a
        // misspelled name here is exactly as invisible as one in the base 24.
        let props = seed(&ProjectScope {
            toolchain: Some(toolchain()),
            ..scope()
        });

        for name in ["VCTargetsPath", "VSInstallDir", "MSBuildExtensionsPath"] {
            let value = props.get(name);
            assert!(value.is_some(), "$({name}) was never seeded");
            assert_ne!(value, Some(""), "$({name}) was seeded empty");
        }
    }

    #[test]
    fn a_property_names_casing_does_not_matter_to_the_seed() {
        // Seeding writes through the same case-insensitive keying MVP-08 built, so a file spelling
        // `$(solutiondir)` — which MSBuild accepts — reaches the seeded slot.
        let props = seed(&scope());

        assert_eq!(props.get("solutiondir"), props.get("SolutionDir"));
        assert_eq!(props.get("MSBUILDTHISFILE"), Some("Lib.vcxproj"));
    }

    #[test]
    fn a_non_ascii_path_is_seeded_intact() {
        // Matching `eval.rs`'s own precedent. Nothing here should be slicing byte offsets, but a
        // path through a user's name is common enough that "should" is not good enough.
        let props = seed(&ProjectScope {
            solution: absolute(&["проект", "App.sln"]),
            project: absolute(&["проект", "lib", "Lib.vcxproj"]),
            config: ConfigPlatform::new("Debug", "x64"),
            toolchain: None,
        });

        assert_property(
            &props,
            "ProjectDir",
            &directory_of(&absolute(&["проект", "lib"])),
        );
        assert_property(
            &props,
            "MSBuildProjectDirectoryNoRoot",
            &relative(&["проект", "lib"]),
        );
    }

    // -------------------------------------------------------------------------------------------
    // `MSBuildThisFile*` — the family that follows the import walk
    // -------------------------------------------------------------------------------------------

    #[test]
    fn this_file_starts_out_pointing_at_the_project() {
        // Seeding it from the project is what spares the walk a special case for its root file, and
        // it is what guarantees `restore` always has a previous value to put back.
        let props = seed(&scope());

        assert_property(&props, "MSBuildThisFile", "Lib.vcxproj");
        assert_property(&props, "MSBuildThisFileName", "Lib");
        assert_property(&props, "MSBuildThisFileExtension", ".vcxproj");
        assert_property(
            &props,
            "MSBuildThisFileFullPath",
            &text(&absolute(&["code", "app", "lib", "Lib.vcxproj"])),
        );
        assert_property(
            &props,
            "MSBuildThisFileDirectory",
            &directory_of(&absolute(&["code", "app", "lib"])),
        );
    }

    #[test]
    fn entering_a_file_repoints_the_whole_this_file_family() {
        // The point of the family: a shared property sheet locates its own siblings with
        // `$(MSBuildThisFileDirectory)`, so pointing it at the root project would send every such
        // path to the wrong directory.
        let mut props = seed(&scope());
        let sheet = absolute(&["code", "app", "shared", "Common.props"]);

        let _outer = enter_file(&mut props, &sheet);

        assert_property(&props, "MSBuildThisFile", "Common.props");
        assert_property(&props, "MSBuildThisFileName", "Common");
        assert_property(&props, "MSBuildThisFileExtension", ".props");
        assert_property(&props, "MSBuildThisFileFullPath", &text(&sheet));
        assert_property(
            &props,
            "MSBuildThisFileDirectory",
            &directory_of(&absolute(&["code", "app", "shared"])),
        );
    }

    #[test]
    fn the_project_family_does_not_follow_the_walk() {
        // `MSBuildProject*` and `Project*` describe the project being evaluated, not the file being
        // read. A sheet that says `$(ProjectDir)` means the `.vcxproj` that pulled it in — which is
        // the entire reason two families exist rather than one.
        let mut props = seed(&scope());
        let before = props.get("MSBuildProjectDirectory").map(str::to_owned);

        let _outer = enter_file(
            &mut props,
            &absolute(&["code", "app", "shared", "Common.props"]),
        );

        assert_eq!(
            props.get("MSBuildProjectDirectory").map(str::to_owned),
            before
        );
        assert_property(&props, "MSBuildProjectFile", "Lib.vcxproj");
        assert_property(
            &props,
            "ProjectDir",
            &directory_of(&absolute(&["code", "app", "lib"])),
        );
    }

    #[test]
    fn restoring_puts_back_the_file_that_was_displaced() {
        let mut props = seed(&scope());
        let sheet = absolute(&["code", "app", "shared", "Common.props"]);

        let outer = enter_file(&mut props, &sheet);
        restore(&mut props, outer);

        assert_property(&props, "MSBuildThisFile", "Lib.vcxproj");
        assert_property(
            &props,
            "MSBuildThisFileDirectory",
            &directory_of(&absolute(&["code", "app", "lib"])),
        );
    }

    #[test]
    fn a_nested_walk_unwinds_one_level_at_a_time() {
        // A sheet that imports a sheet. Restoring to the root rather than to the immediate parent
        // would break the middle file's siblings on the way back out, and a chain ten deep is
        // ordinary in a real project.
        let mut props = seed(&scope());
        let outer_sheet = absolute(&["code", "app", "shared", "Common.props"]);
        let inner_sheet = absolute(&["code", "app", "shared", "warnings", "Strict.props"]);

        let first = enter_file(&mut props, &outer_sheet);
        let second = enter_file(&mut props, &inner_sheet);
        assert_property(&props, "MSBuildThisFile", "Strict.props");

        restore(&mut props, second);
        assert_property(&props, "MSBuildThisFile", "Common.props");
        assert_property(
            &props,
            "MSBuildThisFileDirectory",
            &directory_of(&absolute(&["code", "app", "shared"])),
        );

        restore(&mut props, first);
        assert_property(&props, "MSBuildThisFile", "Lib.vcxproj");
    }

    // -------------------------------------------------------------------------------------------
    // The two tiers
    // -------------------------------------------------------------------------------------------

    #[test]
    fn the_reserved_families_refuse_an_assignment() {
        // MSBuild refuses its own `MSBuild*` properties outright (MSB4004) and drops an assignment
        // to a global one — `Configuration`, `Platform`, `Solution*` under a solution build —
        // without comment. Honouring either would rewrite the paths every include directory is
        // built from.
        let mut props = seed(&scope());

        for name in [
            "MSBuildProjectDirectory",
            "MSBuildThisFileDirectory",
            "MSBuildProjectName",
            "SolutionDir",
            "Configuration",
            "Platform",
        ] {
            let before = props.get(name).map(str::to_owned);
            let expansion = props.assign(name, "hijacked").expect("expands");

            assert!(expansion.refused, "$({name}) must refuse an assignment");
            assert_eq!(
                props.get(name).map(str::to_owned),
                before,
                "$({name}) must keep its seeded value"
            );
        }
    }

    #[test]
    fn the_project_family_accepts_an_assignment() {
        // Not reserved, deliberately. `Microsoft.Cpp.Default.props` defines these conditionally —
        // `<ProjectName Condition="'$(ProjectName)'==''">$(MSBuildProjectName)</ProjectName>` — so
        // a project that renames itself is doing an ordinary thing, and warning about it would
        // teach users to ignore warnings.
        let mut props = seed(&scope());

        let expansion = props.assign("ProjectName", "Renamed").expect("expands");

        assert!(!expansion.refused);
        assert_property(&props, "ProjectName", "Renamed");
        assert_property(&props, "MSBuildProjectName", "Lib");
    }

    #[test]
    fn a_property_the_seed_never_touched_is_ordinary() {
        // Seeding must not lock the bag itself. Everything a project file defines for its own use
        // has to keep working exactly as MVP-08 built it.
        let mut props = seed(&scope());

        let expansion = props
            .assign("MyIncludeRoot", "$(ProjectDir)inc")
            .expect("expands");

        assert!(!expansion.refused);
        assert_property(
            &props,
            "MyIncludeRoot",
            &format!("{}inc", directory_of(&absolute(&["code", "app", "lib"]))),
        );
    }
}
