//! Lexical handling of paths as MSBuild files spell them.
//!
//! Nothing here touches the filesystem, which is the point: a `.vcxproj` may name an include
//! directory that does not exist, and resolving it must still produce the absolute path clangd
//! would look for rather than failing or silently dropping it.

use std::path::{Component, Path, PathBuf};

/// Make `path` absolute and resolve `.` and `..` lexically. Can't use `fs::canonicalize` as that
/// adds a `\\?\` prefix that we don't need to spend extra processing dealing with
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());

    let mut out = PathBuf::new();

    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }

    out
}

/// Converts a path from MSBuild format (canonically windows) to a `PathBuf` the host platform understands.
///
/// Project files write `inc\sub` whatever the platform. Windows accepts `\` and `/` interchangeably
/// but on Linux `PathBuf::from` would keep the whole thing as a single component and CI/WSL would break.
pub(crate) fn from_msbuild(path: &str) -> PathBuf {
    if cfg!(windows) {
        return PathBuf::from(path);
    }
    PathBuf::from(path.replace('\\', "/"))
}

/// Resolve an MSBuild formatted `path` against `base`, absolute and normalized.
///
/// An absolute `path` ignores `base`, which is what `Path::join` already does.
pub(crate) fn resolve(base: &Path, path: &str) -> PathBuf {
    normalize(&base.join(from_msbuild(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// An absolute path built the way the host platform builds one.
    ///
    /// Matching `reserved.rs`'s helper: a `D:\`-prefixed literal is *relative* on the Linux CI
    /// runner, so every expectation has to be constructed rather than written out.
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    #[test]
    fn a_backslash_separates_components_on_every_platform() {
        // The whole reason this function exists. `PathBuf::from(r"inc\sub")` is *one* component on
        // Linux, so a golden file generated in CI would hold a path Windows never produces — and
        // the include directory would be wrong in a way only one platform could see.
        let from_backslash = from_msbuild(r"inc\sub\deep");
        let from_slash = from_msbuild("inc/sub/deep");

        assert_eq!(from_backslash, from_slash);
        assert_eq!(from_backslash.components().count(), 3);
    }

    #[test]
    fn a_single_component_is_unchanged() {
        assert_eq!(from_msbuild("include"), PathBuf::from("include"));
    }

    #[test]
    fn resolving_a_relative_path_hangs_it_off_the_base() {
        let base = absolute(&["code", "app"]);

        assert_eq!(
            resolve(&base, r"inc\public"),
            absolute(&["code", "app", "inc", "public"])
        );
    }

    #[test]
    fn resolving_an_absolute_path_ignores_the_base() {
        // `<AdditionalIncludeDirectories>C:\SDK\include</...>` is unusual but legal, and joining it
        // onto the project directory would produce a path that exists nowhere.
        let base = absolute(&["code", "app"]);
        let elsewhere = absolute(&["vendor", "include"]);

        assert_eq!(
            resolve(&base, &elsewhere.to_string_lossy()),
            elsewhere,
            "an absolute spec replaces the base rather than extending it"
        );
    }

    #[test]
    fn parent_components_are_resolved_lexically() {
        // `..\..\shared\include` is how every multi-project solution reaches a common header
        // directory. ADR 10 settled that `..` is followed rather than refused.
        let base = absolute(&["code", "app", "lib"]);

        assert_eq!(
            resolve(&base, r"..\..\shared\include"),
            absolute(&["code", "shared", "include"])
        );
    }

    #[test]
    fn a_current_directory_component_is_dropped() {
        let base = absolute(&["code", "app"]);

        assert_eq!(
            resolve(&base, r".\inc"),
            absolute(&["code", "app", "inc"]),
            "`.\\inc` and `inc` name the same directory and must produce one spelling"
        );
    }

    #[test]
    fn a_trailing_separator_does_not_survive() {
        // Property sheets write `$(ProjectDir)inc\` as often as `$(ProjectDir)inc`, and two
        // spellings of one directory would defeat the deduplication in `flags.rs`.
        let base = absolute(&["code", "app"]);

        assert_eq!(
            resolve(&base, r"inc\"),
            resolve(&base, "inc"),
            "a trailing separator must not make a second distinct include path"
        );
    }

    #[test]
    fn normalizing_a_relative_path_makes_it_absolute() {
        // `compile_commands.json` consumers require absolute paths (CLAUDE.md §3), and a relative
        // one is the failure that looks like it worked.
        assert!(normalize(Path::new("relative/path")).is_absolute());
    }

    #[test]
    fn climbing_past_the_root_stops_at_the_root() {
        // Hostile input: `..\..\..\..\..\..\..\..` in an include path. ADR 10 removed
        // `Error::PathEscape`, so there is nothing to escape *to* — but it must not panic either,
        // and `PathBuf::pop` at the root is the thing being relied on.
        // Through `resolve`, because `Path::join` on a literal `..\..` would treat the whole thing
        // as one oddly-named directory on Linux and the test would pass without testing anything.
        let normalized = resolve(&absolute(&["code"]), r"..\..\..\..\..\..");

        assert!(
            normalized.is_absolute(),
            "climbing out of the root left {normalized:?}, which is not a usable path"
        );
    }
}
