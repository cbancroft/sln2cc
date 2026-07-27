//! Resolution of `<Import>` elements: `.props` and `.targets` files, including the
//! implicit imports pulled in from the Visual Studio installation.
//!
//! Import order matters — later definitions override earlier ones — so the resolver must
//! preserve the order in which files are pulled into the evaluation.

use std::path::{Path, PathBuf};

use crate::Result;

/// Recursively resolve the import graph rooted at a project or property sheet,
/// returning files in evaluation order.
pub fn resolve(_root: &Path) -> Result<Vec<PathBuf>> {
    todo!("resolve .props/.targets imports")
}
