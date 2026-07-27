//! Real toolchain discovery, backed by `vswhere` (shipped with Visual Studio installers)
//! to locate installed VS instances, then reading the toolset and Windows SDK layout from
//! the installation directory.
//!
//! Windows-only at runtime; the crate as a whole must still build on other platforms.

use crate::toolchain::{Toolchain, ToolchainProvider};
use crate::Result;

#[derive(Debug, Default, Clone)]
pub struct VsWhereToolchain;

impl ToolchainProvider for VsWhereToolchain {
    fn discover(&self) -> Result<Toolchain> {
        todo!("locate VS install via vswhere, resolve toolset + Windows SDK include paths")
    }
}
