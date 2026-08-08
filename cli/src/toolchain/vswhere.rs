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

        // MVP-15: Read the toolset version file under the instance.install_path, locate
        // VC/Tools/MSVC/<version>/include and the Windows SDK's ucrt/um/shared dirs.
        todo!("resolve toolset + Windows SDK include paths from {instance:?}")
    }
}

impl VsWhereToolchain {
    /// Finds the newest installed VS instance carrying the C++ toolset
    fn locate(&self) -> Result<VsInstance> {
        let json = run_vswhere(&vswhere_path())?;
        select_instance(&json)
    }
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
}
