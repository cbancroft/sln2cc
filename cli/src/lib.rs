//! MSVC LSP — parses Visual Studio solutions and generates a `compile_commands.json`
//! compilation database that `clangd` can consume directly.
//!
//! The binary (`main.rs`) is intentionally thin; everything meaningful lives here so that
//! integration tests in `tests/` can exercise it.

pub mod cli;
pub mod compdb;
pub mod db;
pub mod msbuild;
pub mod sln;
pub mod toolchain;

/// Placeholder until a real error enum lands (MVP-02). The plugin has to tell
/// "no Visual Studio installation" apart from "unsupported MSBuild construct" to map
/// failures onto exit codes and JSON, which a boxed trait object can't express without
/// downcasting — hence `thiserror` rather than `anyhow`.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A configuration/platform pair as declared by a solution, e.g. `Debug|x64`.
///
/// Solutions may define custom names, so these are always treated as opaque strings
/// scoped to the solution they came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigPlatform {
    pub configuration: String,
    pub platform: String,
}
