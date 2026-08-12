//! Filesystem expansion of MSBuild's `*`/`?`/`**` wildcard item specs.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::msbuild::paths;
use crate::{Error, Result};

/// Expand one ';' delimited spec element that contains '*' or '?' into the absolute, sorted paths
/// it names.
///
/// `spec` is MSBuild-spelled and resolved against `dir` through the same mechanism a normal spec
/// uses (`paths::resolv`) before any wildcard reading happens. `.`/`..` never need a directory
/// listing so folding them in up front keeps the walk implementation dealing only with real
/// segments. A `Component::Normal` that contains no '*'/'?' is joined directly without a `read_dir`
/// call at all.
///
/// Matching is case-sensitive, following whatever `read_dir` returns.
pub(crate) fn expand(dir: &Path, spec: &str) -> Result<Vec<PathBuf>> {
    let full = paths::resolve(dir, spec);

    let mut root = PathBuf::new();
    let mut segments = Vec::new();

    for component in full.components() {
        match component {
            Component::Normal(part) => segments.push(part.to_string_lossy().into_owned()),
            other => root.push(other), // Prefix/Rootdir; '.'/'..' cannot survive `resolve`
        }
    }

    let mut out = Vec::new();
    walk(&root, &segments, &mut out)?;
    out.sort();
    Ok(out)
}

/// Matche `remaining` against real directory entries under `base`, one segment at a time.
///
/// `**` matches zero or more directory levels, so it tries the rest of the pattern at the current
/// level *and* recurses into every subdirectory keeping `**` alive. A literal segment never lists
/// a directory, it just joins and recurses. The empty `remaining` base case is where a candidate
/// either becomes a match or silently isn't one.
fn walk(base: &Path, remaining: &[String], out: &mut Vec<PathBuf>) -> Result<()> {
    let Some((segment, rest)) = remaining.split_first() else {
        if base.is_file() {
            out.push(base.to_path_buf());
        }
        return Ok(());
    };

    if segment == "**" {
        walk(base, rest, out)?;
        for entry in list(base)? {
            if entry.path().is_dir() {
                walk(&entry.path(), remaining, out)?;
            }
        }

        return Ok(());
    }

    if !has_wildcard(segment) {
        return walk(&base.join(segment), rest, out);
    }

    for entry in list(base)? {
        if matches(&entry.file_name().to_string_lossy(), segment) {
            walk(&entry.path(), rest, out)?;
        }
    }

    Ok(())
}

/// `read_dir` contents with the one error MSBuild treats as "no matches" swallowed.
///
/// Only `NotFound` is silenced, a directory that does not exist matches to nothing. Everything else
/// is forwarded as a real error.
fn list(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .collect::<io::Result<Vec<_>>>()
            .map_err(|err| Error::io(dir, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(Error::io(dir, err)),
    }
}

fn has_wildcard(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?')
}

/// One path sgment against a `*`/`?` pattern. Never crosses a separator since `walk` already
/// operates one segment at a time.
///
/// Uses two-pointer backtracking to do matching rather than recursion.
fn matches(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();

    let (mut ni, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None; // (pattern index after '*', name index tried)

    while ni < name.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == name[ni]) {
            ni += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some((pi + 1, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp;
            ni = sn + 1;
            star = Some((sp, ni))
        } else {
            return false;
        }
    }

    while pattern.get(pi) == Some(&'*') {
        pi += 1;
    }

    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::expand;
    use crate::testing::TempTree;

    #[test]
    fn a_single_star_matches_files_in_one_directory() {
        let dir = TempTree::new("glob-single-star");
        dir.write("src/a.cpp", "");
        dir.write("src/b.cpp", "");
        dir.write("src/c.h", ""); // wrong extension, must not match

        let matched = expand(dir.root(), r"src\*.cpp").expect("expands");

        assert_eq!(matched, [dir.path("src/a.cpp"), dir.path("src/b.cpp")]);
    }

    #[test]
    fn a_question_mark_matches_exactly_one_character() {
        let dir = TempTree::new("glob-question-mark");
        dir.write("test1.cpp", "");
        dir.write("test22.cpp", ""); // two characters where the spec asks for one
        dir.write("testX.cpp", "");

        let matched = expand(dir.root(), "test?.cpp").expect("expands");

        assert_eq!(matched, [dir.path("test1.cpp"), dir.path("testX.cpp")]);
    }

    #[test]
    fn a_double_star_matches_arbitrarily_deep_nesting() {
        // Proves `**` isn't required to cross a directory — `src/top.cpp` matches with zero
        // levels traversed, alongside real nesting at one and two levels.
        let dir = TempTree::new("glob-double-star");
        dir.write("src/top.cpp", "");
        dir.write("src/a/mid.cpp", "");
        dir.write("src/a/b/deep.cpp", "");
        dir.write("src/a/b/skip.h", "");

        let matched = expand(dir.root(), r"src\**\*.cpp").expect("expands");

        assert_eq!(
            matched,
            [
                dir.path("src/a/b/deep.cpp"),
                dir.path("src/a/mid.cpp"),
                dir.path("src/top.cpp"),
            ]
        );
    }

    #[test]
    fn a_wildcard_naming_a_missing_directory_expands_to_nothing() {
        // The issue's explicit requirement: a spec whose directory does not exist is not an
        // error, matching MSBuild rather than failing generation over it.
        let dir = TempTree::new("glob-missing-directory");

        let matched = expand(dir.root(), r"nowhere\*.cpp").expect("does not error");

        assert!(matched.is_empty());
    }

    #[test]
    fn a_wildcard_matching_nothing_expands_to_nothing() {
        let dir = TempTree::new("glob-no-match");
        dir.write("src/a.h", "");

        let matched = expand(dir.root(), r"src\*.cpp").expect("expands");

        assert!(matched.is_empty());
    }

    #[test]
    fn a_directory_entry_is_never_mistaken_for_a_file() {
        let dir = TempTree::new("glob-dir-not-file");
        dir.write("src/real.cpp", "");
        std::fs::create_dir_all(dir.path("src/fake.cpp")).expect("creates the directory");

        let matched = expand(dir.root(), r"src\*.cpp").expect("expands");

        assert_eq!(
            matched,
            [dir.path("src/real.cpp")],
            "a directory whose name matches the pattern is not a source file"
        );
    }

    #[test]
    fn results_are_sorted_regardless_of_directory_order() {
        let dir = TempTree::new("glob-sorted");
        // Written back-to-front, so an unsorted result would come out reversed on a filesystem
        // that happens to preserve creation order in `read_dir`.
        dir.write("src/c.cpp", "");
        dir.write("src/b.cpp", "");
        dir.write("src/a.cpp", "");

        let matched = expand(dir.root(), r"src\*.cpp").expect("expands");

        assert_eq!(
            matched,
            [
                dir.path("src/a.cpp"),
                dir.path("src/b.cpp"),
                dir.path("src/c.cpp"),
            ]
        );
    }

    #[test]
    fn an_absolute_spec_ignores_the_base_directory() {
        let dir = TempTree::new("glob-absolute");
        let elsewhere = TempTree::new("glob-absolute-elsewhere");
        elsewhere.write("vendor.cpp", "");

        let spec = elsewhere.path("*.cpp");
        let matched = expand(dir.root(), &spec.to_string_lossy()).expect("expands");

        assert_eq!(matched, [elsewhere.path("vendor.cpp")]);
    }

    #[test]
    fn a_pathological_pattern_does_not_blow_up() {
        // Many adjacent `*` in one segment is exponential under naive recursive matching.
        // A `.vcxproj` is untrusted input (CLAUDE.md SS5), so this is a cheap guard against
        // reintroducing that algorithm.
        let dir = TempTree::new("glob-pathological");
        dir.write(
            "src/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab.cpp",
            "",
        );

        let pattern = format!(r"src\{}b.cpp", "*".repeat(20));
        let start = std::time::Instant::now();
        let matched = expand(dir.root(), &pattern).expect("expands");

        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "many adjacent '*' must not blow up matching"
        );
        assert_eq!(matched.len(), 1);
    }
}
