//! `compile_commands.json` generation.
//!
//! Flags are emitted in MSVC (`cl`) form and consumed by clangd in clang-cl mode
//! (`--driver-mode=cl`) rather than being translated to native clang flags — translation
//! is the higher-fidelity-risk path.

use std::path::{Path, PathBuf};

use crate::msbuild::EvaluatedProject;
use crate::toolchain::Toolchain;
use crate::Result;

/// A single entry in the compilation database.
#[derive(Debug, Clone)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    pub arguments: Vec<String>,
}

/// Build database entries from evaluated projects plus discovered toolchain include paths.
pub fn build(
    _projects: &[EvaluatedProject],
    _toolchain: &Toolchain,
) -> Result<Vec<CompileCommand>> {
    todo!("build compile_commands entries")
}

/// Write the database to `.msvclsp/<solution-name>/compile_commands.json`.
pub fn write(_commands: &[CompileCommand], _out: &Path) -> Result<()> {
    todo!("serialize compile_commands.json")
}
