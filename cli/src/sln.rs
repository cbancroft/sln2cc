//! `.sln` parsing: enumerate member projects and the solution's declared
//! configuration/platform combinations.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{ConfigPlatform, Error, Result};

/// The project type GUID Visual Studio stamps on every `.vcxproj` member.                                         
//
// Classification is by GUID rather than file extension because that is what Visual Studio
// itself dispatches on. Solution folders (`{2150E333-...}`), C# projects, and `.vcxitems`
// shared-item projects are all *expected* contents of a solution, so they fall out of the
// filter rather than being special-cased or reported.
const VCXPROJ_TYPE_GUID: &str = "{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}";

/// A project referenced by a solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolutionProject {
    pub name: String,
    /// Absolute path to the `.vcxproj`.
    pub path: PathBuf,
    /// The solution-scoped GUID of this project.
    pub guid: String,
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub path: PathBuf,
    pub projects: Vec<SolutionProject>,
    pub configurations: Vec<ConfigPlatform>,
}

/// Parse a `.sln` file.
///
/// Note: `.sln` is a line-oriented text format, not XML — unlike the `.vcxproj`/`.props`
/// files it references.
pub fn parse(path: &Path) -> Result<Solution> {
    let text = std::fs::read_to_string(path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => Error::SolutionNotFound {
            path: path.to_path_buf(),
        },
        _ => Error::io(path, err),
    })?;

    // Visual Studio writes `.sln` as UTF-8 with a BOM, which `read_to_string` keeps as a leading
    // U+FEFF. Harmless to the entry scan, but MVP-06 reads the header line it lands on.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    parse_text(text, path)
}

/// Parsing proper, split from I/O so tests can validate parsing without a file.
///
/// `path` is only used to build absolute project paths and to name the file in errors.
fn parse_text(text: &str, path: &Path) -> Result<Solution> {
    let solution_path = std::path::absolute(path).map_err(|err| Error::io(path, err))?;

    // An absolute path has a parent, unless it is a filesystem root, which cannot be a filename
    // anyway
    let dir = solution_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let mut projects = Vec::new();
    let mut open_entry: Option<usize> = None;

    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();

        if let Some(rest) = line.strip_prefix("Project(") {
            if let Some(opened) = open_entry {
                return Err(malformed(
                    path,
                    line_number,
                    format!("Project entry opened at line {opened} was never closed"),
                ));
            }
            open_entry = Some(line_number);
            if let Some(project) = parse_project_entry(rest, path, line_number, &dir)? {
                projects.push(project);
            }
        } else if line == "EndProject" {
            // Exact match: `EndProjectSection` closes a nested section, not the entry.
            if open_entry.take().is_none() {
                return Err(malformed(
                    path,
                    line_number,
                    "EndProject without a matching Project entry",
                ));
            }
        }
        // MVP-06 adds the `GlobalSection(SolutionConfigurationPlatforms)` and
        // `GlobalSection(ProjectConfigurationPlatforms)` arms here.
    }

    if let Some(opened) = open_entry {
        return Err(malformed(
            path,
            opened,
            "Project entry is missing its EndProject",
        ));
    }

    Ok(Solution {
        path: solution_path,
        projects,
        // MVP-06 fills this in. Nothing reads it until then.
        configurations: Vec::new(),
    })
}

/// Parse what follows `Project(` on an entry line:
///
/// ```text
/// "{TYPE-GUID}") = "Name", "relative/path.vcxproj", "{PROJECT-GUID}"
/// ```
///
/// `Ok(None)` means "not a C++ project" -- A solution folder or a C# project is a normal thing to
/// find in a solution, that is not a failure.
fn parse_project_entry(
    rest: &str,
    sln: &Path,
    line: usize,
    dir: &Path,
) -> Result<Option<SolutionProject>> {
    let fields = quoted_fields(rest);
    let [type_guid, name, location, guid] = fields[..] else {
        return Err(malformed(
            sln,
            line,
            format!(
                "expected 4 quoted fields in Project entry, found {}",
                fields.len()
            ),
        ));
    };

    if !type_guid.eq_ignore_ascii_case(VCXPROJ_TYPE_GUID) {
        return Ok(None);
    }
    if name.is_empty() || location.is_empty() {
        return Err(malformed(
            sln,
            line,
            "Project entry has an empty name or path",
        ));
    }

    Ok(Some(SolutionProject {
        name: name.to_string(),
        path: resolve_path(dir, location),
        guid: guid.to_string(),
    }))
}

/// The `"..."` runs in a line, in order.
///
/// Scanning for quotes rather than matching the full `Project(...) = ..., ...` grammar is much easier.
fn quoted_fields(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut rest = line;

    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else {
            // Unterminated quote drops its field, so the count check catches it.
            break;
        };
        fields.push(&after[..close]);
        rest = &after[close + 1..];
    }

    fields
}

/// Resolve a project location declared in the solution against a solution's directory.
///
/// Split by hand rather than `PathBuf::from`. The solution will always use `\` separators and when
/// the CI runs on Linux `PathBuf::from` will not do the right thing.
fn resolve_path(dir: &Path, location: &str) -> PathBuf {
    let joined = if is_absolute_location(location) {
        // Rare, but legal. Joining it onto `dir` would be incorrect
        PathBuf::from(location.replace('\\', std::path::MAIN_SEPARATOR_STR))
    } else {
        let mut path = dir.to_path_buf();
        for part in location.split(['\\', '/']).filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    };

    normalize(&joined)
}

/// Check whether the location is a full absolute path or not.
///
/// `Path::is_absolute` can't answer this as it is platform dependent.
fn is_absolute_location(location: &str) -> bool {
    matches!(location.as_bytes(), [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic())
        || location.starts_with("\\\\")
        || location.starts_with('/')
}

/// Lexically resolve any `.` and `..` markers in a path name to get the full, final absolute path.
///
/// Avoids using `fs::canonicalize` as that requires the file to exist and on Windows will return
/// a path starting with '\\?\', which we don't want showing up in the final `compile_commands.json`
/// file. `..` is resolved rather than rejected as it is a normal part of a program specification.
/// It is ok for projects to be located out of tree.
fn normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }

    out.into_iter().collect()
}

fn malformed(path: &Path, line: usize, detail: impl Into<String>) -> Error {
    Error::SolutionParse {
        path: path.to_path_buf(),
        line,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::ErrorKind;

    const CPP: &str = "{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}";
    const FOLDER: &str = "{2150E333-8FDC-42A3-9474-1ABA5D1EA1AA}";
    const CSHARP: &str = "{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}";

    /// A rooted solution path for the platform under tests. `parse_text` will absolutize against
    /// the working directory, so `C:\proj\app.sln` will appear to be a *relative* path on Linux CI.
    fn sln() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\proj\app.sln")
        } else {
            PathBuf::from("/proj/app.sln")
        }
    }

    /// Built by joining, so should carry platform specific separator
    fn under_solution(parts: &[&str]) -> PathBuf {
        let mut path = sln().parent().expect("has a parent").to_path_buf();
        parts.iter().for_each(|part| path.push(part));
        path
    }

    #[test]
    fn a_single_project_resolves_against_the_solution_directory() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }
    #[test]
    fn folders_and_non_cpp_projects_are_skipped() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
EndProject
            Project("{FOLDER}") = "app", "app\app.vcxproj", "{{A2}}"
EndProject
            Project("{CSHARP}") = "app", "app\app.vcxproj", "{{A3}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn multiple_projects_keep_declaration_order() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
EndProject
            Project("{CPP}") = "app2", "app\app2.vcxproj", "{{A2}}"
EndProject
            Project("{CPP}") = "app3", "app\app3.vcxproj", "{{A3}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![
                SolutionProject {
                    name: "app".to_string(),
                    path: under_solution(&["app", "app.vcxproj"]),
                    guid: "{A1}".to_string(),
                },
                SolutionProject {
                    name: "app2".to_string(),
                    path: under_solution(&["app", "app2.vcxproj"]),
                    guid: "{A2}".to_string(),
                },
                SolutionProject {
                    name: "app3".to_string(),
                    path: under_solution(&["app", "app3.vcxproj"]),
                    guid: "{A3}".to_string(),
                }
            ]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn a_lowercase_type_guid_is_recognized() {
        let text = format!(
            r#"Project("{}") = "app", "app\app.vcxproj", "{{A1}}"
EndProject
"#,
            CPP.to_lowercase()
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn project_section_lines_are_ignored() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
    ProjectSection(ProjectDependencies)
        {{a}} = {{a}}
    EndProjectSection
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }
    #[test]
    fn a_name_may_contain_a_comma() {
        let text = format!(
            r#"Project("{CPP}") = "app, core", "app\app.vcxproj", "{{A1}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app, core".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn parent_traversal_is_resolved_not_rejected() {
        let text = format!(
            r#"Project("{CPP}") = "app", "..\app\app.vcxproj", "{{A1}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: sln()
                    .parent()
                    .expect("has a parent")
                    .parent()
                    .expect("has a parent")
                    .join("app")
                    .join("app.vcxproj"),
                guid: "{A1}".to_string(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn a_truncated_entry_names_its_line() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
EndProject
Project("{CPP}") = "lib", "lib\lib.vcxproj
EndProject
"#
        );

        let error = parse_text(&text, &sln()).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":3:"), "names the line: {error}");
    }

    #[test]
    fn an_unterminated_project_block_errors() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{{A1}}"
"#
        );

        let error = parse_text(&text, &sln()).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":1:"), "names the line: {error}");
    }

    #[test]
    fn a_missing_solution_is_not_an_io_error() {
        let error = parse(Path::new("no-such-solution.sln")).expect_err("must not parse");
        assert_eq!(error.kind(), ErrorKind::SolutionNotFound);
    }

    #[test]
    fn parent_traversal_past_the_root_stays_literal() {
        // `..\..\..\` climbs past the root. Surplus markers have nothing to pop.
        let text = format!(
            r#"Project("{CPP}") = "app", "..\..\..\app\app.vcxproj", "{{A1}}"
EndProject
"#
        );

        let solution = parse_text(&text, &sln()).expect("parses");

        let resolved = solution.projects[0].path.display().to_string();
        assert!(
            resolved.contains(".."),
            "surplus markers stay literal: {resolved}"
        );
        assert!(
            !resolved.contains("proj\\") && !resolved.contains("proj/"),
            "the solution directory is consumed: {resolved}"
        );
    }
}
