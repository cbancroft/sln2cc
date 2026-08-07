//! `.sln` parsing: enumerate member projects and the solution's declared
//! configuration/platform combinations.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{ConfigPlatform, Error, Reporter, Result, progress};

/// The project type GUID Visual Studio stamps on every `.vcxproj` member.                                         
//
// Classification is by GUID rather than file extension because that is what Visual Studio
// itself dispatches on. Solution folders (`{2150E333-...}`), C# projects, and `.vcxitems`
// shared-item projects are all *expected* contents of a solution, so they fall out of the
// filter rather than being special-cased or reported.
const VCXPROJ_TYPE_GUID: &str = "{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}";

/// The only .`.sln` schema versio this parser supports. VS2013 through VS2022 all write '12.00'. We
/// throw a warning if we run into any other version.
const SUPPORTED_FORMAT_VERSION: &str = "12.00";
const FORMAT_VERSION_PREFIX: &str = "Microsoft Visual Studio Solution File, Format Version ";

/// A project referenced by a solution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolutionProject {
    pub name: String,
    /// Absolute path to the `.vcxproj`.
    pub path: PathBuf,
    /// The solution-scoped GUID of this project.
    pub guid: String,
    /// This project's mapping for each solution config it takes part in, in file order
    pub mappings: Vec<ProjectConfigMapping>,
}

impl SolutionProject {
    /// The mapping to evaluate this project under `solution_config`, or `None` when it takes no
    /// part in that solution configuration.
    ///
    /// Comparison is exact. A case mismatched request is better reported as
    /// [`Error::InvalidConfiguration`], which lists the valid pairs
    pub fn config_for(&self, solution_config: &ConfigPlatform) -> Option<&ProjectConfigMapping> {
        self.mappings
            .iter()
            .find(|mapping| &mapping.solution == solution_config)
    }
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub path: PathBuf,
    pub projects: Vec<SolutionProject>,
    /// The pairs the solution declares, deduplicated and in declaration order
    pub configurations: Vec<ConfigPlatform>,
}

/// The mapping from Project Configuration definition to Solution Configuration.
///
/// The two pairs are separate because they can be two completely separate strings. For example the
/// solution-level `Debug|x64` can mean `Debug|Win32` for a member project. `solution` is what the
/// user picks, `project` is what the MSBuild evaluation uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfigMapping {
    pub solution: ConfigPlatform,
    pub project: ConfigPlatform,
    /// `false` when the solution declares an `ActiveCfg` for this pair but no `Build.0`. The
    /// project is unchecked in the MSVC Configuration Manager and the generator can skip it.
    pub build: bool,
}

/// Which `GlobalSection` the scan is inside. Rows are `key = value` whatever the section, so
/// membership has to be tracked to know what one means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Outside an `GlobalSection` -- where `Project(...)` entries live.
    None,
    SolutionConfigurations,
    ProjectConfigurations,
    /// A section with nothing this parser needs at the moment: `SolutionProperties`,
    /// `NestedProjects`, `ExtensibilityGlobals`, etc. Rows are skipped without inspection.
    Other,
}

/// A parsed `Project(...)` line. Always includes the GUID. Includes the member info when the GUID
/// maps to the C/C++ project. The GUID is always returned even for skipped entries so that
/// duplicate detection catches across project types, where a clash could misattach configuration rows.
struct ProjectEntry {
    guid: String,
    project: Option<SolutionProject>,
}

/// One `ProjectConfigurationPlatforms` row, before it is attached to its corresponding project.
struct ConfigRow {
    guid: String,
    solution: ConfigPlatform,
    project: ConfigPlatform,
    property: RowProperty,
}

/// The trailing component of a row key that this parser acts on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowProperty {
    /// `.ActiveCfg` -- Which project pair the solution maps to
    ActiveCfg,
    /// `.Build.0` -- Presence means the project is built in this solution configuration
    Build,
}

/// Parse a `.sln` file.
///
/// Note: `.sln` is a line-oriented text format, not XML — unlike the `.vcxproj`/`.props`
/// files it references.
pub fn parse_sln(path: &Path, reporter: &mut Reporter) -> Result<Solution> {
    let text = std::fs::read_to_string(path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => Error::SolutionNotFound {
            path: path.to_path_buf(),
        },
        _ => Error::io(path, err),
    })?;

    // Visual Studio writes `.sln` as UTF-8 with a BOM, which `read_to_string` keeps as a leading
    // U+FEFF. Harmless to the entry scan, but MVP-06 reads the header line it lands on.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    parse_sln_text(text, path, reporter)
}

/// Parsing proper, split from I/O so tests can validate parsing without a file.
///
/// `path` is only used to build absolute project paths and to name the file in errors.
fn parse_sln_text(text: &str, path: &Path, reporter: &mut Reporter) -> Result<Solution> {
    let solution_path = std::path::absolute(path).map_err(|err| Error::io(path, err))?;

    // An absolute path has a parent, unless it is a filesystem root, which cannot be a filename
    // anyway
    let dir = solution_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let mut projects: Vec<SolutionProject> = Vec::new();
    let mut configurations: Vec<ConfigPlatform> = Vec::new();
    let mut rows: Vec<ConfigRow> = Vec::new();

    // Every entry's GUID with the line that declared it, so a dupe names both.
    let mut seen_guids: Vec<(String, usize)> = Vec::new();
    let mut format_version: Option<(String, usize)> = None;

    let mut open_entry: Option<usize> = None;
    let mut open_section: Option<usize> = None;
    let mut section = Section::None;

    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();

        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("GlobalSection(") {
            if let Some(opened) = open_section {
                return Err(malformed(
                    path,
                    line_number,
                    format!("GlobalSection opened at line {opened} was never closed."),
                ));
            }
            open_section = Some(line_number);
            section = classify_section(rest);
            continue;
        }

        if line == "EndGlobalSection" {
            if open_section.take().is_none() {
                return Err(malformed(
                    path,
                    line_number,
                    "EndGlobalSection without a matching GlobalSection",
                ));
            }

            section = Section::None;
            continue;
        }

        match section {
            // `Debug|x64 = Debug|x64`. Only the key is read. It is what the rows in
            // ProjectConfigurationPlatforms are spelled with and VS always writes both sides
            // identically.
            Section::SolutionConfigurations => {
                let (key, _) = split_row(line, path, line_number)?;
                let pair = parse_pair(key, path, line_number)?;
                if !configurations.contains(&pair) {
                    configurations.push(pair);
                }
                continue;
            }
            Section::ProjectConfigurations => {
                let (key, value) = split_row(line, path, line_number)?;
                if let Some(row) = parse_config_row(key, value, path, line_number)? {
                    rows.push(row);
                }
                continue;
            }
            Section::Other => continue,
            // Do nothing, continue trying to parse project definitions
            Section::None => {}
        }

        if let Some(rest) = line.strip_prefix("Project(") {
            if let Some(opened) = open_entry {
                return Err(malformed(
                    path,
                    line_number,
                    format!("Project entry opened at line {opened} was never closed"),
                ));
            }
            open_entry = Some(line_number);

            let entry = parse_project_entry(rest, path, line_number, &dir)?;
            if let Some((_, first)) = seen_guids
                .iter()
                .find(|(guid, _)| guid.eq_ignore_ascii_case(&entry.guid))
            {
                return Err(malformed(
                    path,
                    line_number,
                    format!(
                        "duplicate project GUID {}, first seen at line {first}",
                        entry.guid
                    ),
                ));
            }
            seen_guids.push((entry.guid, line_number));
            projects.extend(entry.project);
        } else if line == "EndProject" {
            // Exact match: `EndProjectSection` closes a nested section, not the entry.
            if open_entry.take().is_none() {
                return Err(malformed(
                    path,
                    line_number,
                    "EndProject without a matching Project entry",
                ));
            }
        } else if let Some(version) = line.strip_prefix(FORMAT_VERSION_PREFIX) {
            format_version = Some((version.trim().to_string(), line_number))
        }
    }

    if let Some(opened) = open_entry {
        return Err(malformed(
            path,
            opened,
            "Project entry is missing its EndProject",
        ));
    }

    if let Some(opened) = open_section {
        return Err(malformed(
            path,
            opened,
            "GlobalSection is missing its EndGlobalSection",
        ));
    }

    attach_rows(&mut projects, rows);
    report_version(&solution_path, format_version, reporter);

    progress!(
        reporter,
        "{}: {} C/C++ project(s), {} configuration(s)",
        solution_path.display(),
        projects.len(),
        configurations.len()
    );

    Ok(Solution {
        path: solution_path,
        projects,
        configurations,
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
fn parse_project_entry(rest: &str, sln: &Path, line: usize, dir: &Path) -> Result<ProjectEntry> {
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
        return Ok(ProjectEntry {
            guid: guid.to_string(),
            project: None,
        });
    }
    if name.is_empty() || location.is_empty() {
        return Err(malformed(
            sln,
            line,
            "Project entry has an empty name or path",
        ));
    }

    Ok(ProjectEntry {
        guid: guid.to_string(),
        project: Some(SolutionProject {
            name: name.to_string(),
            path: resolve_path(dir, location),
            guid: guid.to_string(),
            // No mappings yet. These are populated during the config section parsing
            mappings: Vec::new(),
        }),
    })
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

/// Read the section name out of `GlobalSection(<name>) = preSolution`.
///
/// The `presSolution`/`postSolution` token is ignored. It just tell VS where to write the section
/// back out and carries nothing we need.
fn classify_section(rest: &str) -> Section {
    let name = rest
        .split_once(')')
        .map(|(name, _)| name.trim())
        .unwrap_or(rest);

    match name {
        "SolutionConfigurationPlatforms" => Section::SolutionConfigurations,
        "ProjectConfigurationPlatforms" => Section::ProjectConfigurations,
        _ => Section::Other,
    }
}

/// Split a `key = value` row from inside a `GlobalSection`.
///
/// Every row in the two sections read here has this shape. So a line that does not can be
/// considered malformed rather than something to skip. If we just drop the config and report
/// nothing it could lead to silently wrong compile_commands.json files.
fn split_row<'a>(line: &'a str, sln: &Path, number: usize) -> Result<(&'a str, &'a str)> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(malformed(
            sln,
            number,
            "expected a `key = value` row inside a GlobalSection",
        ));
    };
    Ok((key.trim(), value.trim()))
}

/// Parse `Debug|x64` style strings into a ['ConfigPlatform'].
///
/// Both halves are kept verbatim.
fn parse_pair(text: &str, sln_path: &Path, number: usize) -> Result<ConfigPlatform> {
    let Some((config, platform)) = text.split_once('|') else {
        return Err(malformed(
            sln_path,
            number,
            format!("expected `Configuration|Platform`, found {text}"),
        ));
    };

    let (config, platform) = (config.trim(), platform.trim());
    if config.is_empty() || platform.is_empty() {
        return Err(malformed(
            sln_path,
            number,
            format!("`{text}` has an empty configuration or platform"),
        ));
    };

    Ok(ConfigPlatform::new(config, platform))
}

/// Parse a `ProjectConfigurationPlatforms` row:
///
/// ```text
/// {A1}.Debug|x64.ActiveCfg = Debug|Win32
/// {A1}.Debug|x64.Build.0 = Debug|Win32
/// ```
///
/// `Ok(None)` means the key carries a property that this parser has no use for.
fn parse_config_row(
    key: &str,
    value: &str,
    sln: &Path,
    number: usize,
) -> Result<Option<ConfigRow>> {
    let Some((guid, rest)) = split_guid(key) else {
        return Err(malformed(
            sln,
            number,
            format!("row key `{key}` does not start with a project GUID"),
        ));
    };

    // A property outside `ActiveCfg`/`Build.0` means we don't care and can just skip it
    let Some((pair, property)) = split_property(rest) else {
        return Ok(None);
    };

    Ok(Some(ConfigRow {
        guid: guid.to_string(),
        solution: parse_pair(pair, sln, number)?,
        project: parse_pair(value, sln, number)?,
        property,
    }))
}

/// Split a leading `{GUID}.` off a row key.
fn split_guid(key: &str) -> Option<(&str, &str)> {
    if !key.starts_with('{') {
        return None;
    }
    let end = key.find('}')?;
    let rest = key[end + 1..].strip_prefix('.')?;
    Some((&key[..=end], rest))
}

/// Strip a known trailing property off of a key row, leaving the solution-level pair.
///
/// Matched as a suffix rather than splitting on `.` as a configuration name may legitimately
/// contain a dot and splitting would corrupt it.
fn split_property(key: &str) -> Option<(&str, RowProperty)> {
    const PROPERTIES: [(&str, RowProperty); 2] = [
        (".ActiveCfg", RowProperty::ActiveCfg),
        (".Build.0", RowProperty::Build),
    ];

    PROPERTIES
        .iter()
        .find_map(|(suffix, property)| key.strip_suffix(suffix).map(|pair| (pair, *property)))
}

/// Fold collected rows into the projects they name, preserving file order.
///
/// A row naming a GUID no member carries is dropped. It must belong to the solution folder,
/// a different kind of project or another entry the type-GUID filter already excluded.
fn attach_rows(projects: &mut [SolutionProject], rows: Vec<ConfigRow>) {
    for row in rows {
        let Some(project) = projects
            .iter_mut()
            .find(|project| project.guid.eq_ignore_ascii_case(&row.guid))
        else {
            continue;
        };

        // `Build.0` can be read before its `ActiveCfg`, so the pair is upserted rather than
        // assuming `ActiveCfg` created the entry.
        let index = match project
            .mappings
            .iter()
            .position(|mapping| mapping.solution == row.solution)
        {
            Some(index) => index,
            None => {
                project.mappings.push(ProjectConfigMapping {
                    solution: row.solution,
                    project: row.project.clone(),
                    build: false,
                });
                project.mappings.len() - 1
            }
        };

        match row.property {
            RowProperty::ActiveCfg => project.mappings[index].project = row.project,
            RowProperty::Build => project.mappings[index].build = true,
        }
    }
}

/// Warn when the header is absent, or names a schema this parser was not written against.
///
/// These are warnings rather than errors as we assume that we are being handed a solution that VS
/// otherwise opens normally.
fn report_version(path: &Path, version: Option<(String, usize)>, reporter: &mut Reporter) {
    match version {
        Some((version, _)) if version == SUPPORTED_FORMAT_VERSION => {}
        Some((version, line)) => reporter.note_at(
            path,
            Some(line),
            format!("solution format version {version} is not {SUPPORTED_FORMAT_VERSION}; reading it as though it were")
            ),
        None => reporter.warn_at(
            path,
            None,
            "no `Format Version` header. This may not be a solution file."
            ),
    }
}
#[cfg(test)]
mod tests {

    use super::*;
    use crate::{ErrorKind, Severity};

    /// Test project GUIDs
    const A1: &str = "{A1}";
    const B1: &str = "{B1}";
    const C1: &str = "{C1}";
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

    fn solution_text(body: &str) -> String {
        format!("{FORMAT_VERSION_PREFIX}{SUPPORTED_FORMAT_VERSION}\n{body}")
    }

    /// Parse with a quiet reporter, for the majority of tests that assert on the parse itself.
    fn parse(text: &str) -> Result<Solution> {
        parse_sln_text(text, &sln(), &mut Reporter::new(false))
    }

    /// Parse and hand back the reporter, for the tests that assert on what was warned about.
    fn parse_reporting(text: &str) -> (Result<Solution>, Reporter) {
        let mut reporter = Reporter::new(false);
        let result = parse_sln_text(text, &sln(), &mut reporter);
        (result, reporter)
    }

    #[test]
    fn a_single_project_resolves_against_the_solution_directory() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }
    #[test]
    fn folders_and_non_cpp_projects_are_skipped() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
EndProject
            Project("{FOLDER}") = "app", "app\app.vcxproj", "{B1}"
EndProject
            Project("{CSHARP}") = "app", "app\app.vcxproj", "{C1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn multiple_projects_keep_declaration_order() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
EndProject
            Project("{CPP}") = "app2", "app\app2.vcxproj", "{B1}"
EndProject
            Project("{CPP}") = "app3", "app\app3.vcxproj", "{C1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![
                SolutionProject {
                    name: "app".to_string(),
                    path: under_solution(&["app", "app.vcxproj"]),
                    guid: A1.to_string(),
                    mappings: Vec::new(),
                },
                SolutionProject {
                    name: "app2".to_string(),
                    path: under_solution(&["app", "app2.vcxproj"]),
                    guid: B1.to_string(),
                    mappings: Vec::new(),
                },
                SolutionProject {
                    name: "app3".to_string(),
                    path: under_solution(&["app", "app3.vcxproj"]),
                    guid: C1.to_string(),
                    mappings: Vec::new(),
                }
            ]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn a_lowercase_type_guid_is_recognized() {
        let text = format!(
            r#"Project("{}") = "app", "app\app.vcxproj", "{A1}"
EndProject
"#,
            CPP.to_lowercase()
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn project_section_lines_are_ignored() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
    ProjectSection(ProjectDependencies)
        {{a}} = {{a}}
    EndProjectSection
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }
    #[test]
    fn a_name_may_contain_a_comma() {
        let text = format!(
            r#"Project("{CPP}") = "app, core", "app\app.vcxproj", "{A1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects,
            vec![SolutionProject {
                name: "app, core".to_string(),
                path: under_solution(&["app", "app.vcxproj"]),
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn parent_traversal_is_resolved_not_rejected() {
        let text = format!(
            r#"Project("{CPP}") = "app", "..\app\app.vcxproj", "{A1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

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
                guid: A1.to_string(),
                mappings: Vec::new(),
            }]
        );
        assert!(solution.configurations.is_empty(), "MVP-06 fills these");
    }

    #[test]
    fn a_truncated_entry_names_its_line() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
EndProject
Project("{CPP}") = "lib", "lib\lib.vcxproj
EndProject
"#
        );

        let error = parse(&text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":3:"), "names the line: {error}");
    }

    #[test]
    fn an_unterminated_project_block_errors() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
"#
        );

        let error = parse(&text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":1:"), "names the line: {error}");
    }

    #[test]
    fn a_missing_solution_is_not_an_io_error() {
        let error = parse_sln(Path::new("no-such-solution.sln"), &mut Reporter::new(false))
            .expect_err("must not parse");
        assert_eq!(error.kind(), ErrorKind::SolutionNotFound);
    }

    #[test]
    fn parent_traversal_past_the_root_stays_literal() {
        // `..\..\..\` climbs past the root. Surplus markers have nothing to pop.
        let text = format!(
            r#"Project("{CPP}") = "app", "..\..\..\app\app.vcxproj", "{A1}"
EndProject
"#
        );

        let solution = parse(&text).expect("parses");

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

    #[test]
    fn declared_pairs_are_listed_in_declaration_order() {
        let text = solution_text(
            r#"Global
            GlobalSection(SolutionConfigurationPlatforms) = preSolution
            Debug|Win32 = Debug|Win32
            Debug|x64 = Debug|x64
            Release|Win32 = Release|Win32
            Release|x64 = Release|x64
            EndGlobalSection
            EndGlobal
            "#,
        );

        let (result, reporter) = parse_reporting(&text);
        let solution = result.expect("parses");

        assert_eq!(
            solution.configurations,
            vec![
                ConfigPlatform::new("Debug", "Win32"),
                ConfigPlatform::new("Debug", "x64"),
                ConfigPlatform::new("Release", "Win32"),
                ConfigPlatform::new("Release", "x64"),
            ]
        );
        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    #[test]
    fn duplicate_declared_pairs_are_listed_once() {
        let text = solution_text(
            r#"Global
            GlobalSection(SolutionConfigurationPlatforms) = preSolution
            Debug|x64 = Debug|x64
            Debug|x64 = Debug|x64
            EndGlobalSection
            EndGlobal
            "#,
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.configurations,
            vec![ConfigPlatform::new("Debug", "x64")]
        );
    }

    #[test]
    fn a_project_maps_each_declared_pair() {
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(SolutionConfigurationPlatforms) = preSolution
                    Debug|Win32 = Debug|Win32
                    Debug|x64 = Debug|x64
                    Release|Win32 = Release|Win32
                    Release|x64 = Release|x64
                EndGlobalSection
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|Win32.ActiveCfg = Debug|Win32
                    {A1}.Debug|Win32.Build.0 = Debug|Win32
                    {A1}.Debug|x64.ActiveCfg = Debug|x64
                    {A1}.Debug|x64.Build.0 = Debug|x64
                    {A1}.Release|Win32.ActiveCfg = Release|Win32
                    {A1}.Release|Win32.Build.0 = Release|Win32
                    {A1}.Release|x64.ActiveCfg = Release|x64
                    {A1}.Release|x64.Build.0 = Release|x64
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");
        let project = &solution.projects[0];

        assert_eq!(project.mappings.len(), 4);
        for declared in &solution.configurations {
            let mapping = project.config_for(declared).expect("every pair is mapped");
            assert_eq!(
                &mapping.project, declared,
                "identity mapping for {declared}"
            );
            assert!(mapping.build, "{declared} is built");
        }
    }

    #[test]
    fn a_project_pair_may_differ_from_the_solutions() {
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(SolutionConfigurationPlatforms) = preSolution
                    Debug|x64 = Debug|x64
                EndGlobalSection
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.ActiveCfg = Debug|Win32
                    {A1}.Debug|x64.Build.0 = Debug|Win32
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects[0].config_for(&ConfigPlatform::new("Debug", "x64")),
            Some(&ProjectConfigMapping {
                solution: ConfigPlatform::new("Debug", "x64"),
                project: ConfigPlatform::new("Debug", "Win32"),
                build: true
            })
        );
    }

    #[test]
    fn a_custom_configuration_name_is_kept_verbatim() {
        // The name has a space and a dot. This ensures the parser is not thrown off by
        // delimiter assumptions.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(SolutionConfigurationPlatforms) = preSolution
                    Debug With Asserts.2|x64 = Debug With Asserts.2|x64
                EndGlobalSection
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug With Asserts.2|x64.ActiveCfg = Debug With Asserts.2|Win32
                    {A1}.Debug With Asserts.2|x64.Build.0 = Debug With Asserts.2|Win32
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        let custom = ConfigPlatform::new("Debug With Asserts.2", "x64");
        assert_eq!(solution.configurations, vec![custom.clone()]);
        assert_eq!(
            solution.projects[0]
                .config_for(&custom)
                .map(|mapping| &mapping.project),
            Some(&ConfigPlatform::new("Debug With Asserts.2", "Win32"))
        );
    }

    #[test]
    fn a_project_without_a_build_row_is_not_built() {
        // Unchecked in Configuration Manager. The mapping still exists, so generation decides
        // whether to skip it rather than the parser deciding for it.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(SolutionConfigurationPlatforms) = preSolution
                    Debug|x64 = Debug|x64
                    Release|x64 = Release|x64
                EndGlobalSection
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.ActiveCfg = Debug|x64
                    {A1}.Debug|x64.Build.0 = Debug|x64
                    {A1}.Release|x64.ActiveCfg = Release|x64
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");
        let project = &solution.projects[0];

        assert!(
            project
                .config_for(&ConfigPlatform::new("Debug", "x64"))
                .expect("Debug is mapped")
                .build
        );

        let release = project
            .config_for(&ConfigPlatform::new("Release", "x64"))
            .expect("Release is still mapped");
        assert!(!release.build, "no Build.0 row means excluded from build");
        assert_eq!(release.project, ConfigPlatform::new("Release", "x64"));
    }

    #[test]
    fn a_project_absent_from_a_configuration_has_no_mapping() {
        // Distinct from the case above: no rows at all, so the project takes no part in
        // `Release|x64` and there is nothing for generation to evaluate.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(SolutionConfigurationPlatforms) = preSolution
                    Debug|x64 = Debug|x64
                    Release|x64 = Release|x64
                EndGlobalSection
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.ActiveCfg = Debug|x64
                    {A1}.Debug|x64.Build.0 = Debug|x64
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert_eq!(solution.configurations.len(), 2);
        assert_eq!(solution.projects[0].mappings.len(), 1);
        assert!(
            solution.projects[0]
                .config_for(&ConfigPlatform::new("Release", "x64"))
                .is_none()
        );
    }
    #[test]
    fn rows_for_non_cpp_members_are_dropped() {
        // `{B2}` is the C# project the type-GUID filter already excluded. Its rows name a GUID
        // no member carries, which is ordinary solution content, not a fault.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Project("{CSHARP}") = "tool", "tool\tool.csproj", "{B1}"
            EndProject
            Global
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.ActiveCfg = Debug|x64
                    {B1}.Debug|x64.ActiveCfg = Debug|Any CPU
                    {B1}.Debug|x64.Build.0 = Debug|Any CPU
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert_eq!(solution.projects.len(), 1);
        assert_eq!(solution.projects[0].mappings.len(), 1);
    }

    #[test]
    fn other_global_sections_are_ignored() {
        // `NestedProjects` rows are `{GUID} = {GUID}` and `ExtensibilityGlobals` carries a bare
        // key — neither is a configuration, and neither may be read as one.
        let text = solution_text(&format!(
            r#"Project("{FOLDER}") = "src", "src", "{B1}"
            EndProject
            Global
                GlobalSection(SolutionProperties) = preSolution
                    HideSolutionNode = FALSE
                EndGlobalSection
                GlobalSection(NestedProjects) = preSolution
                    {A1} = {B1}
                EndGlobalSection
                GlobalSection(ExtensibilityGlobals) = postSolution
                    SolutionGuid = {A1}
                EndGlobalSection
            EndGlobal
            "#
        ));

        let (result, reporter) = parse_reporting(&text);
        let solution = result.expect("parses");

        assert!(solution.projects.is_empty());
        assert!(solution.configurations.is_empty());
        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    #[test]
    fn an_unknown_row_property_is_skipped() {
        // `.Deploy.0` is meaningless for a native project, and a property this parser cannot
        // name is one it could not act on either. Neither is an error.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.Deploy.0 = Debug|x64
                    {A1}.Debug|x64.SomethingNew = Debug|x64
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert!(solution.projects[0].mappings.is_empty());
    }

    #[test]
    fn rows_may_precede_their_project_entries() {
        // Visual Studio always writes `Global` last. Collecting rows and distributing them
        // after the scan is what keeps the parse from depending on that.
        let text = solution_text(&format!(
            r#"Global
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.ActiveCfg = Debug|Win32
                    {A1}.Debug|x64.Build.0 = Debug|Win32
                EndGlobalSection
            EndGlobal
            Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects[0]
                .config_for(&ConfigPlatform::new("Debug", "x64"))
                .map(|mapping| &mapping.project),
            Some(&ConfigPlatform::new("Debug", "Win32"))
        );
    }

    #[test]
    fn a_build_row_before_its_active_cfg_still_maps() {
        // The pair is upserted, so whichever row arrives first creates it.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Global
                GlobalSection(ProjectConfigurationPlatforms) = postSolution
                    {A1}.Debug|x64.Build.0 = Debug|Win32
                    {A1}.Debug|x64.ActiveCfg = Debug|Win32
                EndGlobalSection
            EndGlobal
            "#
        ));

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.projects[0].config_for(&ConfigPlatform::new("Debug", "x64")),
            Some(&ProjectConfigMapping {
                solution: ConfigPlatform::new("Debug", "x64"),
                project: ConfigPlatform::new("Debug", "Win32"),
                build: true,
            })
        );
    }

    #[test]
    fn tab_indented_rows_parse() {
        // How Visual Studio actually writes it. Not a raw string, so the tabs are visible.
        let text = solution_text(
            "Global\n\tGlobalSection(SolutionConfigurationPlatforms) = preSolution\n\t\tDebug|x64 = Debug|x64\n\tEndGlobalSection\nEndGlobal\n",
        );

        let solution = parse(&text).expect("parses");

        assert_eq!(
            solution.configurations,
            vec![ConfigPlatform::new("Debug", "x64")]
        );
    }

    #[test]
    fn the_header_may_follow_a_leading_blank_line() {
        // Visual Studio writes the BOM, then a blank line, then the header — so the version is
        // found by scanning rather than by reading line 1.
        let text = format!("\n{FORMAT_VERSION_PREFIX}{SUPPORTED_FORMAT_VERSION}\n");

        let (result, reporter) = parse_reporting(&text);
        result.expect("parses");

        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    #[test]
    fn a_malformed_row_names_its_line() {
        // The offending row is line 4 in every case: the header is 1, `Global` is 2, and the
        // `GlobalSection` header is 3.
        let cases: [(&str, &str); 5] = [
            ("SolutionConfigurationPlatforms", "Debug = Debug"),
            ("SolutionConfigurationPlatforms", "Debug| = Debug|"),
            ("SolutionConfigurationPlatforms", "Debug|x64"),
            (
                "ProjectConfigurationPlatforms",
                "Debug|x64.ActiveCfg = Debug|x64",
            ),
            (
                "ProjectConfigurationPlatforms",
                "{A1}.Debug|x64.ActiveCfg = Debug",
            ),
        ];

        for (section, row) in cases {
            let text = solution_text(&format!(
                "Global\n    GlobalSection({section}) = preSolution\n        {row}\n    \
                 EndGlobalSection\nEndGlobal\n"
            ));

            let error = parse(&text).expect_err("must not parse");

            assert_eq!(error.kind(), ErrorKind::SolutionParse, "for `{row}`");
            assert!(
                error.to_string().contains(":4:"),
                "names the line for `{row}`: {error}"
            );
        }
    }

    #[test]
    fn an_unterminated_global_section_names_where_it_opened() {
        // Truncated at EOF, and closed by `EndGlobal` with no `EndGlobalSection`. Both are
        // reported at line 3, where the section opened, not where the file gave up.
        let bodies = [
            "Global\n    GlobalSection(SolutionConfigurationPlatforms) = preSolution\n        \
             Debug|x64 = Debug|x64\n",
            "Global\n    GlobalSection(SolutionConfigurationPlatforms) = preSolution\n        \
             Debug|x64 = Debug|x64\nEndGlobal\n",
        ];

        for body in bodies {
            let error = parse(&solution_text(body)).expect_err("must not parse");

            assert_eq!(error.kind(), ErrorKind::SolutionParse);
        }
    }

    #[test]
    fn a_stray_end_global_section_errors() {
        let error = parse(&solution_text("EndGlobalSection\n")).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":2:"), "names the line: {error}");
    }

    #[test]
    fn a_duplicate_project_guid_names_both_lines() {
        // A C# project sharing a C++ project's GUID is caught too: its rows would otherwise
        // attach to the C++ member, which is why every entry's GUID is tracked, not just ours.
        let text = solution_text(&format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            Project("{CSHARP}") = "tool", "tool\tool.csproj", "{A1}"
            EndProject
            "#
        ));

        let error = parse(&text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::SolutionParse);
        assert!(error.to_string().contains(":4:"), "names the line: {error}");
        assert!(
            error.to_string().contains("first seen at line 2"),
            "names the first entry: {error}"
        );
    }

    /// A *note*, per MVP-13: the header named a schema this parser was not written against, the
    /// file was read as though it were the supported one, and it worked. Nothing about the
    /// resulting flags is in question, which is the whole test ADR 13 applies.
    #[test]
    fn an_unexpected_format_version_notes_and_still_parses() {
        let text = format!(
            r#"{FORMAT_VERSION_PREFIX}11.00
            Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            "#
        );

        let (result, reporter) = parse_reporting(&text);
        let solution = result.expect("parses anyway");

        assert_eq!(solution.projects.len(), 1, "the note is not a refusal");

        let [finding] = reporter.warnings() else {
            panic!("expected exactly one finding: {:?}", reporter.warnings());
        };
        assert_eq!(finding.severity, Severity::Note);
        assert_eq!(finding.line, Some(1));
        assert_eq!(finding.file.as_ref(), Some(&solution.path));
        assert!(
            finding.message.contains("11.00"),
            "names the version: {}",
            finding.message
        );
    }

    /// The other arm stays a *warning*. An arbitrary text file otherwise parses as a solution
    /// with zero projects, reporting nothing — this is the only signal that the input may not be
    /// a solution at all, which is the reason ADR 06 gave for the warning existing.
    #[test]
    fn a_missing_format_version_header_warns() {
        let text = format!(
            r#"Project("{CPP}") = "app", "app\app.vcxproj", "{A1}"
            EndProject
            "#
        );

        let (result, reporter) = parse_reporting(&text);
        result.expect("parses anyway");

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.line, None, "there is no line to name");
    }
}
