//! Discovery of the MSVC toolset and Windows SDK.
//!
//! System include paths and toolset versions come from the Visual Studio installation, not
//! from the project files, so they must be discovered separately.
//!
//! This module is deliberately a trait with swappable implementations: real discovery only
//! works on Windows with VS installed, but unit tests, golden-file tests, and non-Windows
//! CI runners all need deterministic values. Injecting a fake provider is what makes the
//! golden `compile_commands.json` files stable across machines.

pub mod vswhere;

use std::path::PathBuf;

use crate::Result;
/// Toolchain facts that cannot be derived from `.sln`/`.vcxproj` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    /// Visual Studio's own install root. Seeds `$(VSInstallDir)`.
    pub install_path: PathBuf,
    /// `install_path\MSBuild\Microsoft\VC\v{major}0`, precomputed because deriving it needs the
    /// VS *product* version, which `reserved.rs` never sees. Seeds `$(VCTargetsPath)`.
    pub vc_targets_path: PathBuf,
    /// The MSVC toolset include directory, then the Windows SDK's `ucrt`/`um`/`shared`, in the
    /// order `cl` itself searches them.
    pub include_paths: Vec<PathBuf>,
    pub toolset_version: String,
    pub sdk_version: String,
}

pub trait ToolchainProvider {
    fn discover(&self) -> Result<Toolchain>;
}

/// Deterministic provider for tests and non-Windows environments.
#[derive(Debug, Clone)]
pub struct FakeToolchain(pub Toolchain);

impl ToolchainProvider for FakeToolchain {
    fn discover(&self) -> Result<Toolchain> {
        Ok(self.0.clone())
    }
}
