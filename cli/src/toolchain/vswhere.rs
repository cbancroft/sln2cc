//! Real toolchain discovery, backed by `vswhere` (shipped with Visual Studio installers)
//! to locate installed VS instances, then reading the toolset and Windows SDK layout from
//! the installation directory.
//!
//! Windows-only at runtime; the crate as a whole must still build on other platforms.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Error;
use crate::Result;
use crate::toolchain::{Toolchain, ToolchainProvider};

/// The C++ core toolset component. This one also matches "Build Tools for Visual Studio".
const VC_TOOLS_COMPONENT: &str = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64";

/// One Visual Studio instance vswhere reported with the C++ toolset component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsInstance {
    pub install_path: PathBuf,
    pub version: String,
}

#[derive(Debug, Default, Clone)]
pub struct VsWhereToolchain;

impl ToolchainProvider for VsWhereToolchain {
    fn discover(&self) -> Result<Toolchain> {
        let instance = self.locate()?;

        // Read the toolset version file under the instance.install_path, locate
        // VC/Tools/MSVC/<version>/include and the Windows SDK's ucrt/um/shared dirs.
        resolve(&instance, &sdk_root())
    }
}

impl VsWhereToolchain {
    /// Finds the newest installed VS instance carrying the C++ toolset
    fn locate(&self) -> Result<VsInstance> {
        let json = run_vswhere(&vswhere_path())?;
        select_instance(&json)
    }
}

/// Split from `discover` so the disk-walking half is testable without `run_vswhere` or a real
/// `vswhere.exe`.
fn resolve(instance: &VsInstance, sdk_root: &Path) -> Result<Toolchain> {
    let toolset_version = read_toolset_version(&instance.install_path)?;
    let msvc_include = msvc_include_dir(&instance.install_path, &toolset_version)?;
    let sdk = resolve_sdk(sdk_root)?;

    Ok(Toolchain {
        install_path: instance.install_path.clone(),
        vc_targets_path: vc_targets_path(instance),
        include_paths: std::iter::once(msvc_include)
            .chain(sdk.include_dirs())
            .collect(),
        toolset_version,
        sdk_version: sdk.version,
    })
}
/// Reads and trims `VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt`, the file that MSBuild
/// reads to pick a toolset when `$(VCToolsVersion)` isn't overwritten by a project.
fn read_toolset_version(install_path: &Path) -> Result<String> {
    let version_file = install_path
        .join("VC")
        .join("Auxiliary")
        .join("Build")
        .join("Microsoft.VCToolsVersion.default.txt");

    let contents =
        std::fs::read_to_string(&version_file).map_err(|source| Error::ToolchainIncomplete {
            install_root: install_path.to_path_buf(),
            detail: format!("could not read {}: {source}", version_file.display()),
        })?;

    Ok(contents.trim().to_string())
}

/// `install_path\VC\Tools\MSVC\<version>\include` checked to exist. A version the file names but
/// that isn't on disk is a broken install, not a missing property to guess at.
fn msvc_include_dir(install_path: &Path, version: &str) -> Result<PathBuf> {
    let include_dir = install_path
        .join("VC")
        .join("Tools")
        .join("MSVC")
        .join(version)
        .join("include");

    if include_dir.is_dir() {
        Ok(include_dir)
    } else {
        Err(Error::ToolchainIncomplete {
            install_root: install_path.to_path_buf(),
            detail: format!(
                "MSVC include directory not found: {}",
                include_dir.display()
            ),
        })
    }
}

/// `%ProgramFiles(x86)%\Windows Kits\10` -- parallel to vswhere_path(). Same rationale: one known
/// location, no registry and no `PATH` search.
fn sdk_root() -> PathBuf {
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));

    program_files_x86.join("Windows Kits").join("10")
}

#[derive(Debug)]
struct WindowsSdk {
    version: String,
    ucrt: PathBuf,
    um: PathBuf,
    shared: PathBuf,
}

impl WindowsSdk {
    fn include_dirs(&self) -> [PathBuf; 3] {
        [self.ucrt.clone(), self.um.clone(), self.shared.clone()]
    }
}

/// Scans `sdk_root\Include\*`, reusing `parse_version`/numeric comparison and picks the newest
/// version where `ucrt`, `um` and `shared` **all** exist. A version missing one of the three is
/// skipped in favor of an older, complete one rather than failing.
fn resolve_sdk(sdk_root: &Path) -> Result<WindowsSdk> {
    let include_root = sdk_root.join("Include");

    let entries =
        std::fs::read_dir(&include_root).map_err(|source| Error::ToolchainIncomplete {
            install_root: sdk_root.to_path_buf(),
            detail: format!("could not read {}: {source}", include_root.display()),
        })?;

    let mut versions: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();

    versions.sort_by_key(|v| std::cmp::Reverse(parse_version(v)));

    for version in &versions {
        let ucrt = include_root.join(version).join("ucrt");
        let um = include_root.join(version).join("um");
        let shared = include_root.join(version).join("shared");

        if ucrt.is_dir() && um.is_dir() && shared.is_dir() {
            return Ok(WindowsSdk {
                version: version.clone(),
                ucrt,
                um,
                shared,
            });
        }
    }

    Err(Error::ToolchainIncomplete {
        install_root: sdk_root.to_path_buf(),
        detail: format!(
            "no complete Windows SDK (ucrt, um, shared) under {}",
            include_root.display()
        ),
    })
}

/// VS's own targets root: `install_path\MSBuild\Microsoft\VC\v{major}0`, where `major` is
/// `instance.version`'s leading dotted component (`"17.8.x"` -> `v170`).
fn vc_targets_path(instance: &VsInstance) -> PathBuf {
    let version_string = format!(
        "v{}0",
        instance
            .version
            .split_once('.')
            .map(|(major, _)| major)
            .unwrap_or(&instance.version)
    );
    instance
        .install_path
        .join("MSBuild")
        .join("Microsoft")
        .join("VC")
        .join(version_string)
}

/// vswhere's well-known install location, set by every VS/Build Tools installer
fn vswhere_path() -> PathBuf {
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    program_files_x86
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe")
}

/// Shells out to `vswhere`, requesting JSON already filtered to instances with the C++ toolset.
fn run_vswhere(vswhere: &Path) -> Result<String> {
    let output = Command::new(vswhere)
        .args([
            "-products",
            "*",
            "-requires",
            VC_TOOLS_COMPONENT,
            "-format",
            "json",
            "-utf8",
        ])
        .output()
        .map_err(|source| Error::ToolchainNotFound {
            detail: format!("could not run {}: {source}", vswhere.display()),
        })?;

    if !output.status.success() {
        return Err(Error::ToolchainNotFound {
            detail: format!(
                "{} exited with {}: {}",
                vswhere.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }

    String::from_utf8(output.stdout).map_err(|source| Error::ToolchainNotFound {
        detail: format!("vswhere produced non-UTF-8 output: {source}"),
    })
}

/// Parses vswhere's `-format json` output and picks the newest instance. Exists separately from
/// `run_vswhere` for easy testing without filesystem or utility installation.
fn select_instance(json: &str) -> Result<VsInstance> {
    let instances = parse_instances(json)?;
    newest(instances).ok_or_else(|| Error::ToolchainNotFound {
        detail: format!("no Visual Studio instance with {VC_TOOLS_COMPONENT}"),
    })
}

fn parse_instances(json: &str) -> Result<Vec<VsInstance>> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(rename = "installationPath")]
        install_path: PathBuf,
        #[serde(rename = "installationVersion")]
        version: String,
    }

    let raw: Vec<Raw> = serde_json::from_str(json).map_err(|source| Error::ToolchainNotFound {
        detail: format!("could not parse vswhere output: {source}"),
    })?;

    Ok(raw
        .into_iter()
        .map(|r| VsInstance {
            install_path: r.install_path,
            version: r.version,
        })
        .collect())
}

/// Picks the newest dotted version (`17.8.34309.116`), comparing components numerically so that
/// `9.x` never outranks `17.x`. Ties keep the last entry, matching `Iterator::max_by_key`.
fn newest(instances: Vec<VsInstance>) -> Option<VsInstance> {
    instances
        .into_iter()
        .max_by_key(|i| parse_version(&i.version))
}

fn parse_version(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::ErrorKind;
    use crate::testing::TempTree;

    /// Two recorded-shaped instances differing only in `installationVersion`. `select_instance`
    /// must pick the higher one, not the first or last in the array.
    #[test]
    fn multiple_instances_picks_the_newest() {
        let json = r#"[
            {
                "installationPath": "C:\\VS\\2019\\Community",
                "installationVersion": "16.11.34031.279"
            },
            {
                "installationPath": "C:\\VS\\2022\\Community",
                "installationVersion": "17.8.34309.116"
}
        ]"#;

        let instance = select_instance(json).expect("two valid instances must select one");
        assert_eq!(instance.install_path, Path::new(r"C:\VS\2022\Community"));
        assert_eq!(instance.version, "17.8.34309.116");
    }

    /// A single-digit major version must not lexicographically outrank a double-digit one —
    /// `"9.0.0.0" > "17.0.0.0"` as strings, but 17 is the newer release.
    #[test]
    fn newest_compares_versions_numerically_not_lexicographically() {
        let old = VsInstance {
            install_path: PathBuf::from(r"C:\VS\2015"),
            version: "9.0.0.0".to_string(),
        };
        let new = VsInstance {
            install_path: PathBuf::from(r"C:\VS\2022"),
            version: "17.0.0.0".to_string(),
        };

        let picked = newest(vec![old, new.clone()]).expect("two instances must select one");
        assert_eq!(picked, new);
    }

    /// vswhere legitimately returns `[]` when no instance carries the required component —
    /// the normal "no usable installation" case, not a parse failure.
    #[test]
    fn an_empty_result_is_toolchain_not_found() {
        let error = select_instance("[]").expect_err("an empty instance list must be an error");
        assert_eq!(error.kind(), ErrorKind::ToolchainNotFound);
    }

    /// Output that isn't valid JSON at all — a corrupted install, an incompatible vswhere
    /// version — must fail cleanly rather than panicking on the `serde_json` error.
    #[test]
    fn malformed_json_is_toolchain_not_found() {
        let error =
            select_instance("{not valid json").expect_err("malformed output must be an error");
        assert_eq!(error.kind(), ErrorKind::ToolchainNotFound);
    }

    /// `run_vswhere` takes its path as a parameter rather than resolving `vswhere_path()`
    /// internally, specifically so this case — the absence of `vswhere.exe` — is testable on
    /// any OS without a real Visual Studio installation: the path below doesn't exist anywhere.
    #[test]
    fn a_missing_vswhere_binary_is_toolchain_not_found() {
        let missing = Path::new("this/path/does/not/exist/vswhere.exe");
        let error = run_vswhere(missing).expect_err("a missing binary must not panic");
        assert_eq!(error.kind(), ErrorKind::ToolchainNotFound);
    }

    // ---------------------------------------------------------------------------------------
    // MVP-15: resolving the toolset and Windows SDK layout under a located installation
    // ---------------------------------------------------------------------------------------

    /// A tree shaped like `VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt`, with the
    /// trailing CRLF a real file has, so the read-and-trim path has something to trim.
    fn write_toolset_version(tree: &TempTree, version: &str) {
        tree.write(
            "VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt",
            &format!("{version}\r\n"),
        );
    }

    fn make_msvc_include(tree: &TempTree, version: &str) {
        std::fs::create_dir_all(tree.path(&format!("VC/Tools/MSVC/{version}/include")))
            .expect("the temp directory is writable");
    }

    fn make_sdk_family(tree: &TempTree, version: &str, family: &str) {
        std::fs::create_dir_all(tree.path(&format!("Include/{version}/{family}")))
            .expect("the temp directory is writable");
    }

    #[test]
    fn the_default_toolset_version_file_is_read_and_trimmed() {
        let tree = TempTree::new("toolset-version");
        write_toolset_version(&tree, "14.38.33130");

        let version = read_toolset_version(tree.root()).expect("the version file is present");
        assert_eq!(version, "14.38.33130");
    }

    #[test]
    fn a_missing_toolset_version_file_is_toolchain_incomplete() {
        let tree = TempTree::new("toolset-version-missing");

        let error = read_toolset_version(tree.root()).expect_err("no version file must not panic");
        assert_eq!(error.kind(), ErrorKind::ToolchainIncomplete);
    }

    /// Three installed toolsets, version file naming the middle one — proving this reads the
    /// file rather than silently picking the numerically newest directory on disk.
    #[test]
    fn the_matching_msvc_include_directory_is_selected_among_several_installed() {
        let tree = TempTree::new("msvc-include-several");
        for version in ["14.29.30133", "14.38.33130", "14.40.33807"] {
            make_msvc_include(&tree, version);
        }

        let include =
            msvc_include_dir(tree.root(), "14.38.33130").expect("the named version is on disk");

        assert_eq!(include, tree.path("VC/Tools/MSVC/14.38.33130/include"));
    }

    #[test]
    fn an_msvc_include_directory_missing_for_the_named_version_is_toolchain_incomplete() {
        let tree = TempTree::new("msvc-include-missing");
        make_msvc_include(&tree, "14.29.30133");

        let error = msvc_include_dir(tree.root(), "14.38.33130")
            .expect_err("a version the file names but that isn't installed must not panic");
        assert_eq!(error.kind(), ErrorKind::ToolchainIncomplete);
    }

    #[test]
    fn resolve_sdk_picks_the_newest_version_with_all_three_families_present() {
        let tree = TempTree::new("sdk-newest");
        for version in ["10.0.19041.0", "10.0.22621.0"] {
            for family in ["ucrt", "um", "shared"] {
                make_sdk_family(&tree, version, family);
            }
        }

        let sdk = resolve_sdk(tree.root()).expect("two complete SDKs must select one");
        assert_eq!(sdk.version, "10.0.22621.0");
    }

    /// A half-installed newest SDK is a real disk state, not a project file lying — the older
    /// complete SDK is usable and should win rather than failing generation outright.
    #[test]
    fn a_version_missing_one_sdk_family_is_skipped_in_favor_of_an_older_complete_one() {
        let tree = TempTree::new("sdk-incomplete-newest");
        for family in ["ucrt", "shared"] {
            make_sdk_family(&tree, "10.0.22621.0", family);
        }
        for family in ["ucrt", "um", "shared"] {
            make_sdk_family(&tree, "10.0.19041.0", family);
        }

        let sdk = resolve_sdk(tree.root()).expect("an older complete SDK must still be found");
        assert_eq!(sdk.version, "10.0.19041.0");
    }

    #[test]
    fn no_windows_sdk_installed_is_toolchain_incomplete() {
        let tree = TempTree::new("sdk-absent");

        let error = resolve_sdk(tree.root()).expect_err("an empty SDK root must not panic");
        assert_eq!(error.kind(), ErrorKind::ToolchainIncomplete);
    }

    /// The VS *product* version, not the MSVC toolset version, decides this folder — the two
    /// numbers are unrelated (e.g. VS 17.8.x ships MSVC toolset 14.38.x).
    #[test]
    fn vc_targets_path_maps_the_product_version_to_its_v_folder() {
        let cases = [("17.8.34309.116", "v170"), ("16.11.34031.279", "v160")];

        for (version, folder) in cases {
            let instance = VsInstance {
                install_path: PathBuf::from(r"C:\VS\2022\Community"),
                version: version.to_string(),
            };

            let path = vc_targets_path(&instance);
            assert_eq!(
                path,
                PathBuf::from(r"C:\VS\2022\Community")
                    .join("MSBuild")
                    .join("Microsoft")
                    .join("VC")
                    .join(folder),
                "{version} must map to {folder}"
            );
        }
    }

    #[test]
    fn discover_combines_toolset_and_sdk_into_ordered_include_paths() {
        let tree = TempTree::new("discover-combined");
        write_toolset_version(&tree, "14.38.33130");
        make_msvc_include(&tree, "14.38.33130");
        for family in ["ucrt", "um", "shared"] {
            make_sdk_family(&tree, "10.0.22621.0", family);
        }

        let instance = VsInstance {
            install_path: tree.root().to_path_buf(),
            version: "17.8.34309.116".to_string(),
        };

        let toolchain = resolve(&instance, tree.root()).expect("a complete fake tree resolves");

        assert_eq!(
            toolchain.include_paths,
            vec![
                tree.path("VC/Tools/MSVC/14.38.33130/include"),
                tree.path("Include/10.0.22621.0/ucrt"),
                tree.path("Include/10.0.22621.0/um"),
                tree.path("Include/10.0.22621.0/shared"),
            ]
        );
        assert_eq!(toolchain.toolset_version, "14.38.33130");
        assert_eq!(toolchain.sdk_version, "10.0.22621.0");
        assert_eq!(toolchain.install_path, tree.root());
    }
}
