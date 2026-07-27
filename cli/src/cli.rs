//! Subcommand definitions and the JSON output types that form the contract with the
//! Neovim plugin.
//!
//! Anything the plugin consumes programmatically must be emitted as structured JSON,
//! never human-formatted text.

use std::path::PathBuf;

use crate::ConfigPlatform;

/// Result of the `discover` subcommand.
#[derive(Debug, Clone)]
pub struct DiscoveredSolution {
    pub path: PathBuf,
    /// Name used to namespace artifacts under `.msvclsp/<name>/`.
    pub name: String,
}

/// Result of the `list-configs` subcommand.
#[derive(Debug, Clone)]
pub struct SolutionConfigs {
    pub solution: PathBuf,
    pub available: Vec<ConfigPlatform>,
    /// Last-used selection for this solution, if one has been recorded.
    pub active: Option<ConfigPlatform>,
}
