//! MSBuild evaluation for `.vcxproj` files: the most complex part of the system.
//!
//! A `.vcxproj` is not self-contained — it imports `.props`/`.targets` files (including
//! from the Visual Studio installation), inherits properties through property sheets, and
//! gates values behind `Condition` attributes. Producing correct flags means evaluating
//! that graph for a specific configuration/platform.
//!
//! Target schema: Visual Studio 2019 and 2022. Older toolsets are out of scope for v1.

pub mod eval;
pub mod imports;
pub mod project;
pub mod reserved;

use std::path::{Path, PathBuf};

use crate::{ConfigPlatform, Result};

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

/// Evaluate a `.vcxproj` for the given configuration/platform.
pub fn evaluate(_project: &Path, _config: &ConfigPlatform) -> Result<EvaluatedProject> {
    todo!("evaluate .vcxproj")
}
