//! Throwaway directory trees for the tests that need real files on disk.
//!
//! Most of this crate's tests parse text and assert on the result, and those stay in the module
//! they cover. Import resolution and full-project evaluation cannot: they read files, so they need
//! files. This lived inside `msbuild/imports.rs`'s test module until `msbuild.rs` became a second
//! caller.
//!
//! Hand-rolled rather than added as a dev-dependency: `Cargo.toml` records the dependency list as
//! settled, and this is forty lines of `std::fs`. `fixtures/` is not the alternative — MVP-23 and
//! MVP-24 own filling it, and a corpus meant for manual clangd verification is no place for an
//! import cycle or a deliberately broken project.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A directory removed when the test that made it ends.
pub(crate) struct TempTree(PathBuf);

impl TempTree {
    /// A fresh directory named for the calling test.
    ///
    /// The process id and a counter keep concurrent test binaries and parallel tests within one
    /// from colliding; the name makes a leftover directory traceable if a run is killed.
    pub(crate) fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let path = std::env::temp_dir().join(format!(
            "sln2cc-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the temp directory is writable");
        TempTree(path)
    }

    /// Write a project file at `relative`, wrapping `body` in a `<Project>` element.
    ///
    /// `<Project>` is line 1, so **the body starts at line 2** — which is what every line-number
    /// assertion counts from. The namespace is omitted; MVP-07 made it optional.
    pub(crate) fn project(&self, relative: &str, body: &str) -> PathBuf {
        self.write(relative, &format!("<Project>\n{body}\n</Project>\n"))
    }

    pub(crate) fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the temp directory is writable");
        }
        std::fs::write(&path, contents).expect("the temp directory is writable");
        path
    }

    /// `relative` is spelled with `/`, which both platforms accept.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    /// The tree's own directory, for stripping a prefix off an absolute result.
    pub(crate) fn root(&self) -> &Path {
        &self.0
    }

    /// Force `relative`'s mtime to an exact value, for tests asserting staleness detection. Two
    /// successive `write` calls can land within the same filesystem timestamp tick, which would
    /// make a "touch and expect staleness" test flaky without this.
    pub(crate) fn set_mtime(&self, relative: &str, time: std::time::SystemTime) {
        let file = std::fs::File::options()
            .write(true)
            .open(self.path(relative))
            .expect("the temp tree file exists and is writable");
        file.set_modified(time)
            .expect("set_modified is supported on the test platform");
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
