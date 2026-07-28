//! JSON-file persistence.
//!
//! Stores, per discovered solution: its known configuration/platform values, its own
//! last-used selection, and source/project file modification timestamps used to decide
//! whether a reparse is needed.
//!
//! State is scoped per-solution from the start. Only one solution is active at a time in
//! v1, but keeping the layout solution-scoped is what allows multiple simultaneously
//! active solutions later without a migration.
//!
//! SQLite was the original plan and was dropped: the payload is a few KB, and the only
//! practical way to link SQLite on Windows is to compile the bundled C amalgamation, which
//! would make a C toolchain a hard build requirement. The tradeoff accepted in exchange is
//! that concurrent writers are last-writer-wins rather than transactional — see
//! docs/engineering_spec.md. Writes must therefore be atomic (write a temporary file, then
//! rename over the target) so a crash or a concurrent run can never leave a torn file.
//!
//! The on-disk shape is an open question — see docs/engineering_spec.md.

use std::path::{Path, PathBuf};

use crate::{ConfigPlatform, Result};

/// Handle to the state file stored under `.msvclsp/`.
#[derive(Debug)]
pub struct Store {
    pub path: PathBuf,
}

impl Store {
    /// Open (creating if absent) the state file for a project root.
    pub fn open(_msvclsp_dir: &Path) -> Result<Self> {
        todo!("load or initialize the json state file")
    }

    /// Record the last-used configuration/platform for a solution.
    pub fn set_active_config(&self, _solution: &Path, _config: &ConfigPlatform) -> Result<()> {
        todo!("persist active config/platform")
    }

    /// Fetch a solution's own last-used configuration/platform, if recorded.
    pub fn active_config(&self, _solution: &Path) -> Result<Option<ConfigPlatform>> {
        todo!("read active config/platform")
    }

    /// True when nothing the solution depends on has changed since the last parse.
    pub fn is_up_to_date(&self, _solution: &Path) -> Result<bool> {
        todo!("compare stored mtimes")
    }
}
