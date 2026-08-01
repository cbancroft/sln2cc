//! MSVC LSP — parses Visual Studio solutions and generates a `compile_commands.json`
//! compilation database that `clangd` can consume directly.
//!
//! The binary (`main.rs`) is intentionally thin; everything meaningful lives here so that
//! integration tests in `tests/` can exercise it.

pub mod cli;
pub mod compdb;
pub mod db;
pub mod error;
pub mod msbuild;
pub mod sln;
pub mod toolchain;

pub use error::{Error, ErrorKind, ResultExt};

/// The crate's fallible return type. Every error carries an [`ErrorKind`], which is what the
/// plugin dispatches on — see [`error`] for why that isn't a boxed trait object.
pub type Result<T> = std::result::Result<T, Error>;

/// A configuration/platform pair as declared by a solution, e.g. `Debug|x64`.
///
/// Solutions may define custom names, so these are always treated as opaque strings
/// scoped to the solution they came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigPlatform {
    pub configuration: String,
    pub platform: String,
}

/// Renders as `Debug|x64`, the form solutions and project conditions both use, so error
/// messages and pickers name a pair the way the user's own files do.
impl std::fmt::Display for ConfigPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}|{}", self.configuration, self.platform)
    }
}
