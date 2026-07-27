//! `.sln` parsing: enumerate member projects and the solution's declared
//! configuration/platform combinations.

use std::path::{Path, PathBuf};

use crate::{ConfigPlatform, Result};

/// A project referenced by a solution.
#[derive(Debug, Clone)]
pub struct SolutionProject {
    pub name: String,
    /// Absolute path to the `.vcxproj`.
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub path: PathBuf,
    pub projects: Vec<SolutionProject>,
    pub configurations: Vec<ConfigPlatform>,
}

/// Parse a `.sln` file.
///
/// Note: `.sln` is a line-oriented text format, not XML — unlike the `.vcxproj`/`.props`
/// files it references.
pub fn parse(_path: &Path) -> Result<Solution> {
    todo!("parse .sln")
}
