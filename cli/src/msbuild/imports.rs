//! Resolution of `<Import>` elements: `.props` and `.targets` files, including the
//! implicit imports pulled in from the Visual Studio installation.
//!
//! This is MSBuilds **first** evaluation pass. Import paths are macro-expanded and condition-gated
//! so resolution cannot run as a pre-pass over the graph.
//!
//! Items and item definitions are collected but deliberately not evaluated. MSBuild settles these
//! in a later pass against the property values the whole graph ended with.
//!
//! Import order matters — later definitions override earlier ones — so the resolver must
//! preserve the order in which files are pulled into the evaluation.

use std::path::{Path, PathBuf};

use crate::msbuild::eval::{self, Properties};
use crate::msbuild::glob;
use crate::msbuild::paths::normalize;
use crate::msbuild::project::{self, Element, Import, PropertyGroup};
use crate::msbuild::reserved;
use crate::{Error, Reporter, Result, progress};

/// How deep an import chain may go before it is refused.
///
/// A real chain runs ~10-15 files deep so this is generous
const MAX_IMPORT_DEPTH: usize = 64;

/// Everything MSBuild's first pass produces for one project.
#[derive(Debug, Clone)]
pub struct ProjectTree {
    /// Every file entered, in the order evaluation entered it, root first.
    pub files: Vec<PathBuf>,
    /// Item definitions and item groups from every file, spliced into one stream in evaluation
    /// order. Conditions are unevaluated at this point. See [`Deferred`].
    pub deferred: Vec<Deferred>,
    /// The property bag as the whole graph left it.
    pub properties: Properties,
}

/// An element the first pass does not consume, and the file it came from.
///
/// The `Condition` is deliberately left unevaluated. MSBuild decides which item definition and
/// items apply in a later pass, against the property values evaluation *ended* with instead of the
/// ones in scope where the element was written. MVP-12 will evaluate them against
/// [`ProjectTree::properties`].
#[derive(Debug, Clone)]
pub struct Deferred {
    /// Relative item paths resolve against this file's directory, and diagnostics name it.
    pub file: PathBuf,
    /// An [`Element::ItemDefinitions`] or an [`Element::Items`].
    pub element: Element,
}

/// Walk `root` and everything it imports, in evaluation order.
///
/// `properties` is passed in, rather than built here, so that [`crate::msbuild::reserved::seed`]
/// can supply the reserved properties without this function having to know what they are. An empty
/// bag is also a valid starting point — a reference nothing seeded expands to nothing, which is
/// still what `$(VCTargetsPath)` does until MVP-15 supplies the Visual Studio paths.
///
/// The one property family this function does own is `MSBuildThisFile*`, which is a function of the
/// file being read rather than of the project, and so has to follow the walk.
pub fn resolve(
    root: &Path,
    properties: Properties,
    reporter: &mut Reporter,
) -> Result<ProjectTree> {
    let mut walk = Walk {
        files: Vec::new(),
        deferred: Vec::new(),
        properties,
        depth: 0,
        reporter,
    };

    walk.enter(&normalize(root), None)?;

    Ok(ProjectTree {
        files: walk.files,
        deferred: walk.deferred,
        properties: walk.properties,
    })
}

/// The state carried down the import graph.
///
/// `files` doubles as the visited set.
struct Walk<'a> {
    files: Vec<PathBuf>,
    deferred: Vec<Deferred>,
    properties: Properties,
    depth: usize,
    reporter: &'a mut Reporter,
}

impl Walk<'_> {
    /// Evaluate one file, following its imports inline. `path` must already be normalized.
    fn enter(&mut self, path: &Path, referrer: Option<&Path>) -> Result<()> {
        self.depth += 1;
        let outer = reserved::enter_file(&mut self.properties, path);

        let result = self.evaluate(path, referrer);

        reserved::restore(&mut self.properties, outer);
        self.depth -= 1;
        result
    }

    fn evaluate(&mut self, path: &Path, referrer: Option<&Path>) -> Result<()> {
        if self.depth > MAX_IMPORT_DEPTH {
            return Err(Error::RecursionLimit {
                limit: MAX_IMPORT_DEPTH,
                context: format!("importing {}", path.display()),
            });
        }

        // MSBuild imports a file once per project.
        if self.files.iter().any(|seen| same_file(seen, path)) {
            self.reporter.note_at(
                referrer.unwrap_or(path),
                None,
                format!(
                    "`{}` was already imported and will not be imported again",
                    path.display()
                ),
            );
            return Ok(());
        }

        // Only an import can be missing. A referrer is what makes the error nameable.
        if let Some(referrer) = referrer {
            if !path.is_file() {
                return Err(Error::MissingImport {
                    import: path.to_path_buf(),
                    referenced_by: referrer.to_path_buf(),
                });
            }
        }

        progress!(self.reporter, "evaluating {}", path.display());

        // Recorded before parsing so a file that imports itself is caught by the above check
        self.files.push(path.to_path_buf());
        let file = project::parse_project(path, self.reporter)?;

        for element in file.elements {
            match element {
                Element::Properties(group) => self.property_group(group, path)?,
                Element::Import(import) => self.import(&import, path)?,
                Element::ImportGroup(group) => {
                    if self.holds(group.condition.as_deref(), path, group.line)? {
                        for import in &group.imports {
                            self.import(import, path)?;
                        }
                    }
                }
                element @ (Element::Items(_) | Element::ItemDefinitions(_)) => {
                    self.deferred.push(Deferred {
                        file: path.to_path_buf(),
                        element,
                    });
                }
                Element::Unsupported { .. } => {}
            }
        }

        Ok(())
    }

    fn property_group(&mut self, group: PropertyGroup, file: &Path) -> Result<()> {
        if !self.holds(group.condition.as_deref(), file, group.line)? {
            return Ok(());
        }

        for property in group.properties {
            if !self.holds(property.condition.as_deref(), file, property.line)? {
                continue;
            }

            let expansion = self.properties.assign(&property.name, &property.value)?;
            // Read before reporting as `report` consumes.
            let refused = expansion.refused;
            expansion.report(self.reporter, file, property.line);

            if refused {
                self.reporter.warn_at(
                    file,
                    Some(property.line),
                    format!(
                        "`{}` is a reserved MSBuild property; assignment ignored",
                        property.name
                    ),
                );
            }
        }

        Ok(())
    }

    fn import(&mut self, import: &Import, file: &Path) -> Result<()> {
        // The condition comes first on purpose. A false one is how sheet guards an optional import
        // and resolving the path first would mistakenly report a missing file.
        if !self.holds(import.condition.as_deref(), file, import.line)? {
            return Ok(());
        }

        let value = eval::expand(&import.project, &self.properties)?.report(
            self.reporter,
            file,
            import.line,
        );

        let target = value.trim();
        if target.is_empty() {
            self.reporter.warn_at(
                file,
                Some(import.line),
                "`<Import>` resolved to an empty path and was skipped",
            );
            return Ok(());
        }

        // Parsing happens relative to the importing files own directory as resolved and not the root's.
        let base = file.parent().unwrap_or(Path::new(""));

        if target.contains('*') || target.contains('?') {
            return self.import_wildcard(target, base, file, import.line);
        }
        self.enter(&normalize(&base.join(target)), Some(file))
    }

    /// Evaluate a `Condition`, reporting what it could not resolve. An absent condition still holds.
    fn holds(&mut self, condition: Option<&str>, file: &Path, line: usize) -> Result<bool> {
        let Some(condition) = condition else {
            return Ok(true);
        };

        Ok(
            eval::condition_holds(condition, &self.properties, file, line)?.report(
                self.reporter,
                file,
                line,
            ),
        )
    }

    /// Import every file a wildcarded `Project` spec names, in [`glob::expand`]'s order.
    ///
    /// Reuses `enter` per match rather than a custom loop. Unlike a literal target, a wildcard
    /// resolving to zero matches is not `Error::MissingImport`. It is just a warning because we
    /// still want to let the user know that a Condition gated import resolved to nothing.
    fn import_wildcard(
        &mut self,
        target: &str,
        base: &Path,
        file: &Path,
        line: usize,
    ) -> Result<()> {
        let matched = glob::expand(base, target)?;

        if matched.is_empty() {
            self.reporter.warn_at(
                file,
                Some(line),
                format!("`<Import Project=\"{target}\">` matched no files"),
            );
            return Ok(());
        }

        for path in matched {
            self.enter(&path, Some(file))?;
        }

        Ok(())
    }
}

/// Whether two normalized paths name the same file (case insensitive on Windows)
fn same_file(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        return left.as_os_str().eq_ignore_ascii_case(right.as_os_str());
    }

    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msbuild::reserved;
    use crate::testing::TempTree;
    use crate::{ConfigPlatform, ErrorKind};

    /// Resolve from an empty bag with a quiet reporter, for the cases asserting on the walk.
    fn tree(root: &Path) -> ProjectTree {
        resolve(root, Properties::new(), &mut Reporter::new(false)).expect("resolves")
    }

    /// Resolve and hand back the reporter, for the cases asserting on what was warned about.
    fn tree_reporting(root: &Path) -> (Result<ProjectTree>, Reporter) {
        let mut reporter = Reporter::new(false);
        let result = resolve(root, Properties::new(), &mut reporter);
        (result, reporter)
    }

    /// A bag seeded the way the generation pipeline will seed it, for the MVP-11 cases.
    ///
    /// The `.sln` is named but never written: seeding reads the path and nothing else, and a
    /// solution file that exists would only invite an `Exists()` case to pass for the wrong reason.
    fn seeded(dir: &TempTree, root: &Path) -> Properties {
        reserved::seed(&reserved::ProjectScope {
            solution: dir.path("App.sln"),
            project: root.to_path_buf(),
            config: ConfigPlatform::new("Debug", "x64"),
            toolchain: None,
        })
    }

    /// Resolve from a seeded bag, for the cases about the reserved properties.
    fn tree_seeded(dir: &TempTree, root: &Path) -> ProjectTree {
        let properties = seeded(dir, root);
        resolve(root, properties, &mut Reporter::new(false)).expect("resolves")
    }

    /// Resolve from a seeded bag and hand back the reporter.
    fn tree_seeded_reporting(dir: &TempTree, root: &Path) -> (Result<ProjectTree>, Reporter) {
        let properties = seeded(dir, root);
        let mut reporter = Reporter::new(false);
        let result = resolve(root, properties, &mut reporter);
        (result, reporter)
    }

    /// The value the walk left in `name`, for the properties each case has a file record into.
    fn value<'a>(resolved: &'a ProjectTree, name: &str) -> &'a str {
        resolved
            .properties
            .get(name)
            .unwrap_or_else(|| panic!("$({name}) was never assigned"))
    }

    /// The directory `file` sits in, spelled with a trailing separator as MSBuild spells it.
    ///
    /// Built through `normalize` so the expectation is spelled exactly the way the walk spells the
    /// path it derived its value from — `TempTree` writes relative paths with `/`, which Windows
    /// accepts on the way in and does not preserve on the way out.
    fn this_file_directory(file: &Path) -> String {
        let normalized = normalize(file);
        let parent = normalized.parent().expect("a file has a directory");
        format!("{}{}", parent.display(), std::path::MAIN_SEPARATOR)
    }

    fn refuse(root: &Path) -> Error {
        resolve(root, Properties::new(), &mut Reporter::new(false)).expect_err("must not resolve")
    }

    /// The visited files, relative to the tree root and with `/` separators, so assertions read
    /// the same on either platform.
    fn names(resolved: &ProjectTree, dir: &TempTree) -> Vec<String> {
        resolved
            .files
            .iter()
            .map(|file| {
                file.strip_prefix(dir.root())
                    .unwrap_or(file)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    /// The `PreprocessorDefinitions` of each deferred item definition, in order — which is what the
    /// ordering tests label their groups with.
    fn defines(resolved: &ProjectTree) -> Vec<&str> {
        resolved
            .deferred
            .iter()
            .filter_map(|deferred| match &deferred.element {
                Element::ItemDefinitions(group) => Some(group),
                _ => None,
            })
            .flat_map(|group| &group.definitions)
            .flat_map(|definition| &definition.metadata)
            .map(|setting| setting.value.as_str())
            .collect()
    }

    #[test]
    fn a_linear_chain_is_entered_in_order() {
        let dir = TempTree::new("linear");
        let root = dir.project("root.vcxproj", r#"  <Import Project="a.props" />"#);
        dir.project("a.props", r#"  <Import Project="b.props" />"#);
        dir.project("b.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "a.props", "b.props"]
        );
    }

    #[test]
    fn an_import_path_is_expanded_against_properties_defined_above_it() {
        // Why resolution cannot be a pre-pass: the path is not knowable until the properties
        // written above it have been assigned.
        let dir = TempTree::new("expanded-path");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <SheetDir>sheets</SheetDir>
  </PropertyGroup>
  <Import Project="$(SheetDir)/common.props" />"#,
        );
        dir.project("sheets/common.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "sheets/common.props"]
        );
    }

    #[test]
    fn a_diamond_imports_the_shared_sheet_once() {
        // MSBuild's MSB4011 rule. Evaluating `d.props` twice would re-run every `$(X);append`
        // inside it, so the shared sheet is entered where it is first reached and skipped after.
        let dir = TempTree::new("diamond");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="b.props" />
  <Import Project="c.props" />"#,
        );
        dir.project("b.props", r#"  <Import Project="d.props" />"#);
        dir.project("c.props", r#"  <Import Project="d.props" />"#);
        dir.project("d.props", "  <PropertyGroup />");

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("resolves");

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "b.props", "d.props", "c.props"]
        );

        let [finding] = reporter.warnings() else {
            panic!("expected exactly one finding: {:?}", reporter.warnings());
        };
        assert!(
            finding.message.contains("already imported"),
            "says why it was skipped: {}",
            finding.message
        );
        // A *note*, per MVP-13: MSBuild's MSB4011 rule says to import a file once per project,
        // and that is exactly what happened. Nothing was lost, so nothing about the flags is in
        // question — and a diamond is common enough that warning would swamp the channel.
        assert_eq!(finding.severity, crate::Severity::Note);
    }

    #[test]
    fn a_cycle_terminates_by_the_duplicate_rule() {
        // The todo asked for an error here. Under MSBuild's own rule there is nothing left to
        // detect: the second `a.props` is simply a repeat import, and the walk stops. See ADR 10.
        let dir = TempTree::new("cycle");
        let root = dir.project("a.props", r#"  <Import Project="b.props" />"#);
        dir.project("b.props", r#"  <Import Project="a.props" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("must terminate rather than erroring");

        assert_eq!(names(&resolved, &dir), ["a.props", "b.props"]);
        assert_eq!(reporter.warnings().len(), 1);
    }

    #[test]
    fn a_file_that_imports_itself_is_skipped() {
        // Only caught because the file is recorded before it is parsed, not after.
        let dir = TempTree::new("self-import");
        let root = dir.project("a.props", r#"  <Import Project="a.props" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("must terminate rather than erroring");

        assert_eq!(names(&resolved, &dir), ["a.props"]);
        assert_eq!(reporter.warnings().len(), 1);
    }

    #[test]
    fn a_conditional_import_that_is_false_is_never_resolved() {
        // `absent.props` does not exist. The condition is evaluated before the path so that a
        // guarded optional import never reports a missing file the project deliberately allows for
        // — which is exactly how every Visual Studio project guards `*.user.props`.
        let dir = TempTree::new("false-condition");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="absent.props" Condition="'$(Flavour)'=='fancy'" />"#,
        );

        let resolved = tree(&root);

        assert_eq!(names(&resolved, &dir), ["root.vcxproj"]);
    }

    #[test]
    fn a_missing_required_import_names_the_referrer() {
        // The error ADR 07 reserved for this issue: the reader cannot raise it, because it does
        // not know which file pointed at the one that is missing.
        let dir = TempTree::new("missing-import");
        let root = dir.project("root.vcxproj", r#"  <Import Project="absent.props" />"#);

        let error = refuse(&root);

        assert_eq!(error.kind(), ErrorKind::MissingImport);
        let message = error.to_string();
        assert!(
            message.contains("absent.props"),
            "names the import: {message}"
        );
        assert!(
            message.contains("root.vcxproj"),
            "names the referrer: {message}"
        );
    }

    #[test]
    fn the_root_being_absent_is_an_io_error() {
        // Not `MissingImport`: there is no referrer to name, so it falls through to the reader.
        let dir = TempTree::new("absent-root");

        let error = refuse(&dir.path("no-such-project.vcxproj"));

        assert_eq!(error.kind(), ErrorKind::Io);
    }

    #[test]
    fn an_import_group_condition_gates_its_children() {
        // Neither `debug.props` nor `extra.props` exists, so reaching either would fail the walk.
        // The group's condition and each child's are separate gates, which is what ADR 07 kept
        // them apart for.
        let dir = TempTree::new("import-group");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <Configuration>Release</Configuration>
  </PropertyGroup>
  <ImportGroup Condition="'$(Configuration)'=='Debug'">
    <Import Project="debug.props" />
  </ImportGroup>
  <ImportGroup Condition="'$(Configuration)'=='Release'">
    <Import Project="release.props" />
    <Import Project="extra.props" Condition="'$(Extras)'=='on'" />
  </ImportGroup>"#,
        );
        dir.project("release.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(names(&resolved, &dir), ["root.vcxproj", "release.props"]);
    }

    #[test]
    fn a_property_defined_in_an_import_is_visible_after_it() {
        // The whole point of the pass: an imported sheet's properties are in scope for everything
        // written below the `<Import>`.
        let dir = TempTree::new("import-defines");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="a.props" />
  <PropertyGroup>
    <Combined>$(FromImport)/local</Combined>
  </PropertyGroup>"#,
        );
        dir.project(
            "a.props",
            r#"  <PropertyGroup>
    <FromImport>imported</FromImport>
  </PropertyGroup>"#,
        );

        let resolved = tree(&root);

        assert_eq!(resolved.properties.get("Combined"), Some("imported/local"));
    }

    #[test]
    fn a_definition_after_an_import_shadows_the_imported_one() {
        // Document order across the whole graph, not per file: the import wins over what preceded
        // it, and what follows the import wins over the import.
        let dir = TempTree::new("shadowing");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <Shared>before</Shared>
  </PropertyGroup>
  <Import Project="a.props" />
  <PropertyGroup>
    <SeenAfterImport>$(Shared)</SeenAfterImport>
    <Shared>after</Shared>
  </PropertyGroup>"#,
        );
        dir.project(
            "a.props",
            r#"  <PropertyGroup>
    <Shared>imported</Shared>
  </PropertyGroup>"#,
        );

        let resolved = tree(&root);

        assert_eq!(
            resolved.properties.get("SeenAfterImport"),
            Some("imported"),
            "the import replaced the root's earlier value"
        );
        assert_eq!(resolved.properties.get("Shared"), Some("after"));
    }

    #[test]
    fn deferred_elements_keep_true_evaluation_order() {
        // The reason this pass returns a spliced stream rather than a list of files. A `.vcxproj`
        // writes its `ItemDefinitionGroup`s between `Microsoft.Cpp.props` and
        // `Microsoft.Cpp.targets`, so grouping by file would put the root's ahead of the imported
        // sheet's where MSBuild puts them after — wrong on the ordinary layout.
        let dir = TempTree::new("deferred-order");
        let root = dir.project(
            "root.vcxproj",
            r#"  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>ROOT_BEFORE</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>
  <Import Project="a.props" />
  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>ROOT_AFTER</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>"#,
        );
        dir.project(
            "a.props",
            r#"  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>IMPORTED</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>"#,
        );

        let resolved = tree(&root);

        assert_eq!(
            defines(&resolved),
            ["ROOT_BEFORE", "IMPORTED", "ROOT_AFTER"]
        );
        assert_eq!(
            resolved.deferred[1].file, resolved.files[1],
            "each deferred element names the file it came from"
        );
    }

    #[test]
    fn deferred_conditions_are_left_unevaluated() {
        // Kept despite the condition being false right now. MSBuild settles item definitions in a
        // later pass, against the property values the whole graph ended with, so this call belongs
        // to MVP-12 and answering it here would answer a different question.
        let dir = TempTree::new("deferred-conditions");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <Configuration>Release</Configuration>
  </PropertyGroup>
  <ItemDefinitionGroup Condition="'$(Configuration)'=='Debug'">
    <ClCompile>
      <PreprocessorDefinitions>DEBUG_ONLY</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>"#,
        );

        let resolved = tree(&root);

        assert_eq!(defines(&resolved), ["DEBUG_ONLY"]);
        let Element::ItemDefinitions(group) = &resolved.deferred[0].element else {
            panic!(
                "expected an ItemDefinitionGroup: {:?}",
                resolved.deferred[0]
            );
        };
        assert_eq!(
            group.condition.as_deref(),
            Some("'$(Configuration)'=='Debug'"),
            "carried through verbatim"
        );
    }

    #[test]
    fn a_relative_import_resolves_against_its_own_files_directory() {
        // Against the importing file, not the root. Resolving against the root would look for
        // `b.props` beside `root.vcxproj` and fail — and a property sheet importing a sibling is
        // the whole reason a shared sheets directory works.
        let dir = TempTree::new("relative-base");
        let root = dir.project("root.vcxproj", r#"  <Import Project="sub/a.props" />"#);
        dir.project("sub/a.props", r#"  <Import Project="b.props" />"#);
        dir.project("sub/b.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "sub/a.props", "sub/b.props"]
        );
    }

    #[test]
    fn a_parent_traversal_is_followed_and_normalised() {
        // `..` is how multi-project solutions share a sheet, so it is followed rather than
        // refused. Normalising is what makes `sub/../shared.props` and `shared.props` count as one
        // file for the duplicate rule.
        let dir = TempTree::new("parent-traversal");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="shared.props" />
  <Import Project="sub/a.props" />"#,
        );
        dir.project("shared.props", "  <PropertyGroup />");
        dir.project("sub/a.props", r#"  <Import Project="../shared.props" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("resolves");

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "shared.props", "sub/a.props"]
        );
        assert_eq!(
            reporter.warnings().len(),
            1,
            "the traversal reached a file already imported: {:?}",
            reporter.warnings()
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_separator_resolves() {
        // The spelling real project files use. Everything else here writes `/`, which both
        // platforms accept, so this is the one case covering what Visual Studio actually emits.
        let dir = TempTree::new("windows-separator");
        let root = dir.project("root.vcxproj", r#"  <Import Project="sub\a.props" />"#);
        dir.project("sub/a.props", r#"  <Import Project="..\shared.props" />"#);
        dir.project("shared.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(
            names(&resolved, &dir),
            ["root.vcxproj", "sub/a.props", "shared.props"]
        );
    }

    #[test]
    fn an_import_resolving_to_an_empty_path_is_skipped_with_a_warning() {
        // What every real `.vcxproj` does today, until MVP-15 supplies `$(VCTargetsPath)` — the one
        // family MVP-11's seed deliberately left out. Skipping beats resolving to the filesystem
        // root.
        let dir = TempTree::new("empty-path");
        let root = dir.project("root.vcxproj", r#"  <Import Project="$(VCTargetsPath)" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("resolves");

        assert_eq!(names(&resolved, &dir), ["root.vcxproj"]);
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("empty path")),
            "{:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn an_import_chain_deeper_than_the_cap_is_refused() {
        // Not about cycles — the duplicate rule handles those. This is a generated tree of
        // distinct files deep enough to exhaust the stack.
        let dir = TempTree::new("deep-chain");
        let last = MAX_IMPORT_DEPTH + 5;
        for step in 0..last {
            dir.project(
                &format!("p{step}.props"),
                &format!(r#"  <Import Project="p{}.props" />"#, step + 1),
            );
        }
        dir.project(&format!("p{last}.props"), "  <PropertyGroup />");

        let error = refuse(&dir.path("p0.props"));

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn a_chain_at_the_cap_still_resolves() {
        // The cap is a ceiling, not a limit below it.
        let dir = TempTree::new("chain-at-cap");
        let last = MAX_IMPORT_DEPTH - 1;
        for step in 0..last {
            dir.project(
                &format!("p{step}.props"),
                &format!(r#"  <Import Project="p{}.props" />"#, step + 1),
            );
        }
        dir.project(&format!("p{last}.props"), "  <PropertyGroup />");

        let resolved = tree(&dir.path("p0.props"));

        assert_eq!(resolved.files.len(), MAX_IMPORT_DEPTH);
    }

    // -------------------------------------------------------------------------------------------
    // MVP-41 — wildcard `<Import Project="...">` expansion
    // -------------------------------------------------------------------------------------------

    #[test]
    fn a_wildcard_import_matches_several_files_in_order() {
        // Order asserted, not just membership: `glob::expand`'s filesystem-sorted order is what
        // MSBuild itself uses for a wildcard import, and a diamond-style dedup bug could still
        // pass a membership-only check.
        let dir = TempTree::new("wildcard-import-order");
        let root = dir.project("root.vcxproj", r#"  <Import Project="hooks\*.props" />"#);
        dir.project("hooks/c.props", "  <PropertyGroup />");
        dir.project("hooks/a.props", "  <PropertyGroup />");
        dir.project("hooks/b.props", "  <PropertyGroup />");

        let resolved = tree(&root);

        assert_eq!(
            names(&resolved, &dir),
            [
                "root.vcxproj",
                "hooks/a.props",
                "hooks/b.props",
                "hooks/c.props",
            ]
        );
    }

    #[test]
    fn a_wildcard_import_matching_nothing_warns() {
        // Unlike a literal target, a wildcard matching zero files is not `Error::MissingImport` —
        // nothing in the graph named one specific missing file. It still warns: a
        // `Condition="Exists(...)"`-gated hook directory that resolved to nothing is a real
        // signal worth surfacing, per MVP-41's decision.
        let dir = TempTree::new("wildcard-import-empty");
        let root = dir.project("root.vcxproj", r#"  <Import Project="hooks\*.props" />"#);
        dir.write("hooks/readme.txt", ""); // present, but nothing matches `*.props`

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("a zero-match wildcard is not an error");

        assert_eq!(names(&resolved, &dir), ["root.vcxproj"]);
        let [finding] = reporter.warnings() else {
            panic!("expected exactly one finding: {:?}", reporter.warnings());
        };
        assert!(
            finding.message.contains("hooks") && finding.message.contains("*.props"),
            "names the spec that matched nothing: {}",
            finding.message
        );
        assert_eq!(finding.severity, crate::Severity::Warning);
    }

    #[test]
    fn a_wildcard_import_whose_directory_does_not_exist_does_not_error() {
        // Mirrors `glob.rs`'s own `a_wildcard_naming_a_missing_directory_expands_to_nothing`,
        // exercised through the import path: a wildcard whose directory is entirely absent must
        // not be confused with a real I/O failure, matching ADR 29's "missing directory expands
        // to nothing" rule reused here rather than reimplemented.
        let dir = TempTree::new("wildcard-import-missing-dir");
        let root = dir.project("root.vcxproj", r#"  <Import Project="nowhere\*.props" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("a missing directory is not an error");

        assert_eq!(names(&resolved, &dir), ["root.vcxproj"]);
        assert_eq!(reporter.warnings().len(), 1, "{:?}", reporter.warnings());
    }

    #[test]
    fn a_cycle_reached_only_through_a_wildcard_import_is_deduped() {
        // ADR 10's MSB4011 dedup applies per matched file, not per spec: `hooks/a.props` imports
        // `root.vcxproj` back, but `root.vcxproj` is already in `self.files` by the time the
        // wildcard reaches it, so the walk terminates rather than looping or erroring.
        let dir = TempTree::new("wildcard-import-cycle");
        let root = dir.project("root.vcxproj", r#"  <Import Project="hooks\*.props" />"#);
        dir.project("hooks/a.props", r#"  <Import Project="..\root.vcxproj" />"#);

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("must terminate rather than erroring");

        assert_eq!(names(&resolved, &dir), ["root.vcxproj", "hooks/a.props"]);
        let [finding] = reporter.warnings() else {
            panic!("expected exactly one finding: {:?}", reporter.warnings());
        };
        assert!(
            finding.message.contains("already imported"),
            "says why it was skipped: {}",
            finding.message
        );
        assert_eq!(finding.severity, crate::Severity::Note);
    }

    #[test]
    fn an_unsupported_condition_on_an_import_is_fatal() {
        // ADR 09's policy, inherited here. `&lt;` is how a `<` survives an XML attribute, so this
        // is the spelling a real file would carry.
        let dir = TempTree::new("unsupported-condition");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="a.props" Condition="'1' &lt; '2'" />"#,
        );
        dir.project("a.props", "  <PropertyGroup />");

        let error = refuse(&root);

        assert_eq!(error.kind(), ErrorKind::UnsupportedConstruct);
    }

    #[test]
    fn undefined_and_unsupported_findings_reach_the_reporter() {
        // The bridge. MVP-08 and MVP-09 record findings without warning because neither knows
        // where its input was written; this pass is the first code holding both.
        let dir = TempTree::new("findings");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <IncludePath>$(Missing);$([System.IO.Path]::GetTempPath())</IncludePath>
  </PropertyGroup>"#,
        );

        let (result, reporter) = tree_reporting(&root);
        result.expect("resolves");

        let messages: Vec<&str> = reporter
            .warnings()
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("Missing")),
            "the undefined property is reported: {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("System.IO.Path")),
            "the unsupported construct is reported: {messages:?}"
        );
        assert!(
            reporter
                .warnings()
                .iter()
                .all(|warning| warning.line == Some(3) && warning.file.as_deref() == Some(&root)),
            "each finding is sited where it was written: {:?}",
            reporter.warnings()
        );
        // Both are warnings, not notes. An unresolved reference is how an include path quietly
        // loses a directory, and MVP-08 copies an unsupported construct through verbatim so it
        // reaches a flag as written — either way the flags may now be wrong. See ADR 13.
        assert!(
            reporter
                .warnings()
                .iter()
                .all(|warning| warning.severity == crate::Severity::Warning),
            "{:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn an_unsupported_element_does_not_stop_the_walk() {
        // `<Choose>` is recorded and warned about by the reader. This pass has nothing to do with
        // it, and must not treat it as a reason to stop.
        let dir = TempTree::new("unsupported-element");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="a.props" />
  <PropertyGroup>
    <AfterChoose>reached</AfterChoose>
  </PropertyGroup>"#,
        );
        dir.project(
            "a.props",
            r#"  <Choose>
    <When Condition="'$(X)'=='y'">
      <PropertyGroup>
        <Bits>64</Bits>
      </PropertyGroup>
    </When>
  </Choose>"#,
        );

        let (result, reporter) = tree_reporting(&root);
        let resolved = result.expect("resolves");

        assert_eq!(names(&resolved, &dir), ["root.vcxproj", "a.props"]);
        assert_eq!(resolved.properties.get("AfterChoose"), Some("reached"));
        assert_eq!(
            resolved.properties.get("Bits"),
            None,
            "the nested `PropertyGroup` was not hoisted out of the `When`"
        );
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("Choose")),
            "{:?}",
            reporter.warnings()
        );
    }

    // -------------------------------------------------------------------------------------------
    // MVP-11 — the seeded bag, and `MSBuildThisFile*` following the walk
    // -------------------------------------------------------------------------------------------

    #[test]
    fn this_file_directory_differs_between_the_project_and_an_imported_sheet() {
        // The property that makes a shared sheet able to locate its own siblings, and the reason
        // seeding cannot be a single up-front call: it has to follow the walk. A sheet one directory
        // down that saw the root project's directory would send every relative path it builds to a
        // place with none of its headers in it.
        let dir = TempTree::new("this-file-differs");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <FromRoot>$(MSBuildThisFileDirectory)</FromRoot>
  </PropertyGroup>
  <Import Project="sheets/common.props" />"#,
        );
        let sheet = dir.project(
            "sheets/common.props",
            r#"  <PropertyGroup>
    <FromSheet>$(MSBuildThisFileDirectory)</FromSheet>
  </PropertyGroup>"#,
        );

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(value(&resolved, "FromRoot"), this_file_directory(&root));
        assert_eq!(value(&resolved, "FromSheet"), this_file_directory(&sheet));
        assert_ne!(
            value(&resolved, "FromRoot"),
            value(&resolved, "FromSheet"),
            "the two files sit in different directories"
        );
    }

    #[test]
    fn this_file_reverts_to_the_importing_file_when_an_import_returns() {
        // The other half of following the walk, and the one an up-front seed would get wrong in the
        // direction nobody notices: everything after the `<Import>` in the root file would silently
        // resolve against the last sheet the walk happened to enter.
        let dir = TempTree::new("this-file-reverts");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="sheets/common.props" />
  <PropertyGroup>
    <AfterImport>$(MSBuildThisFileDirectory)</AfterImport>
    <AfterImportFile>$(MSBuildThisFile)</AfterImportFile>
  </PropertyGroup>"#,
        );
        dir.project("sheets/common.props", "  <PropertyGroup />");

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(value(&resolved, "AfterImport"), this_file_directory(&root));
        assert_eq!(value(&resolved, "AfterImportFile"), "root.vcxproj");
    }

    #[test]
    fn this_file_unwinds_correctly_through_a_nested_import() {
        // Restoring to the root rather than to the immediate parent would break the middle file's
        // siblings on the way back out, which a two-level chain is the shortest way to catch.
        let dir = TempTree::new("this-file-nested");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="sheets/outer.props" />"#,
        );
        let outer = dir.project(
            "sheets/outer.props",
            r#"  <Import Project="strict/inner.props" />
  <PropertyGroup>
    <BackInOuter>$(MSBuildThisFileDirectory)</BackInOuter>
  </PropertyGroup>"#,
        );
        let inner = dir.project(
            "sheets/strict/inner.props",
            r#"  <PropertyGroup>
    <InInner>$(MSBuildThisFileDirectory)</InInner>
  </PropertyGroup>"#,
        );

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(value(&resolved, "InInner"), this_file_directory(&inner));
        assert_eq!(value(&resolved, "BackInOuter"), this_file_directory(&outer));
    }

    #[test]
    fn the_project_family_stays_fixed_on_the_root_project_throughout_the_walk() {
        // `$(ProjectDir)` in a sheet means the `.vcxproj` that pulled the sheet in, never the sheet
        // itself. It is the property nearly every `AdditionalIncludeDirectories` is built from, so
        // having it drift with the walk would misplace most of a real database's include paths.
        let dir = TempTree::new("project-family-fixed");
        let root = dir.project(
            "root.vcxproj",
            r#"  <Import Project="sheets/common.props" />"#,
        );
        dir.project(
            "sheets/common.props",
            r#"  <PropertyGroup>
    <SheetSawProjectDir>$(ProjectDir)</SheetSawProjectDir>
    <SheetSawProjectName>$(MSBuildProjectName)</SheetSawProjectName>
  </PropertyGroup>"#,
        );

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(
            value(&resolved, "SheetSawProjectDir"),
            this_file_directory(&root)
        );
        assert_eq!(value(&resolved, "SheetSawProjectName"), "root");
    }

    #[test]
    fn a_seeded_solution_dir_resolves_an_import_path() {
        // What MVP-10 could only assert was skipped. A sheet held at the solution root and pulled in
        // through `$(SolutionDir)` is how a multi-project solution shares settings, and against an
        // empty bag the path collapsed to a bare relative one.
        let dir = TempTree::new("seeded-solution-dir");
        let root = dir.project(
            "projects/app/app.vcxproj",
            r#"  <Import Project="$(SolutionDir)shared/common.props" />"#,
        );
        dir.project(
            "shared/common.props",
            r#"  <PropertyGroup>
    <Reached>yes</Reached>
  </PropertyGroup>"#,
        );

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(resolved.files.len(), 2, "{:?}", resolved.files);
        assert_eq!(value(&resolved, "Reached"), "yes");
    }

    #[test]
    fn a_condition_can_test_a_seeded_directory_with_exists() {
        // `Exists()` had nothing real to test against an empty bag. This is the idiom every Visual
        // Studio project guards `Microsoft.Cpp.$(Platform).user.props` with, and the seeded value is
        // what finally makes the true branch reachable.
        let dir = TempTree::new("seeded-exists");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup Condition="Exists('$(SolutionDir)shared')">
    <SharedExists>yes</SharedExists>
  </PropertyGroup>
  <PropertyGroup Condition="Exists('$(SolutionDir)absent')">
    <AbsentExists>yes</AbsentExists>
  </PropertyGroup>"#,
        );
        dir.project("shared/common.props", "  <PropertyGroup />");

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(value(&resolved, "SharedExists"), "yes");
        assert_eq!(
            resolved.properties.get("AbsentExists"),
            None,
            "a directory that is not there must not test true"
        );
    }

    #[test]
    fn a_seeded_configuration_pair_decides_which_property_group_applies() {
        // The whole point of the exercise, end to end: the pair the user picked reaches the
        // condition every real `.vcxproj` is organised around. Against an empty bag both sides read
        // `'|'` and the wrong group could win.
        let dir = TempTree::new("seeded-config");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'">
    <Defines>_DEBUG</Defines>
  </PropertyGroup>
  <PropertyGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'">
    <Defines>NDEBUG</Defines>
  </PropertyGroup>"#,
        );

        let resolved = tree_seeded(&dir, &root);

        assert_eq!(value(&resolved, "Defines"), "_DEBUG");
    }

    #[test]
    fn a_conditional_default_for_a_seeded_property_is_skipped_without_a_warning() {
        // `<Configuration Condition="'$(Configuration)'==''">Debug</Configuration>` opens a large
        // share of real `.vcxproj` files. The condition is false once the property is seeded, so the
        // assignment is never attempted and the lock is never consulted — which is what keeps the
        // refusal warning off ordinary files.
        let dir = TempTree::new("conditional-default");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <Configuration Condition="'$(Configuration)'==''">Release</Configuration>
  </PropertyGroup>"#,
        );

        let (result, reporter) = tree_seeded_reporting(&dir, &root);
        let resolved = result.expect("resolves");

        assert_eq!(value(&resolved, "Configuration"), "Debug");
        assert!(
            reporter.warnings().is_empty(),
            "an idiomatic conditional default is not worth a warning: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn an_unconditional_assignment_to_a_reserved_property_is_ignored_and_warned_about() {
        // MSBuild refuses this outright. Warning and carrying on keeps the database generatable
        // while naming the file that tried, which is the split ADR 11 settles: honouring it would
        // rewrite the paths every include directory in the project is built from.
        let dir = TempTree::new("reserved-assignment");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <MSBuildProjectDirectory>C:\hijacked</MSBuildProjectDirectory>
  </PropertyGroup>"#,
        );

        let (result, reporter) = tree_seeded_reporting(&dir, &root);
        let resolved = result.expect("resolves");

        assert_ne!(
            resolved.properties.get("MSBuildProjectDirectory"),
            Some(r"C:\hijacked"),
            "the seeded value must survive"
        );

        let warning = reporter
            .warnings()
            .iter()
            .find(|warning| warning.message.contains("MSBuildProjectDirectory"))
            .unwrap_or_else(|| panic!("no warning named the property: {:?}", reporter.warnings()));
        assert!(
            warning.message.contains("reserved"),
            "the warning says why: {}",
            warning.message
        );
        assert_eq!(warning.line, Some(3), "sited where it was written");
        let expected = normalize(&root);
        assert_eq!(warning.file.as_deref(), Some(expected.as_path()));
    }

    #[test]
    fn a_project_can_still_override_the_overridable_seeds() {
        // `ProjectName` is seeded but not locked, because MSBuild's own `.props` define it
        // conditionally and a project renaming itself is ordinary. Warning about it would train
        // users to ignore the warning that matters.
        let dir = TempTree::new("overridable-seed");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <ProjectName>Renamed</ProjectName>
  </PropertyGroup>"#,
        );

        let (result, reporter) = tree_seeded_reporting(&dir, &root);
        let resolved = result.expect("resolves");

        assert_eq!(value(&resolved, "ProjectName"), "Renamed");
        assert_eq!(
            value(&resolved, "MSBuildProjectName"),
            "root",
            "the reserved spelling is unaffected"
        );
        assert!(
            reporter.warnings().is_empty(),
            "overriding an overridable seed is not a finding: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn an_empty_bag_is_still_a_valid_starting_point() {
        // Seeding is the caller's to do, not this module's, and every case above this section
        // resolves from an empty bag. Nothing about MVP-11 may make one mandatory.
        let dir = TempTree::new("unseeded-still-works");
        let root = dir.project(
            "root.vcxproj",
            r#"  <PropertyGroup>
    <FromRoot>$(MSBuildThisFile)</FromRoot>
  </PropertyGroup>"#,
        );

        let resolved = tree(&root);

        assert_eq!(
            value(&resolved, "FromRoot"),
            "root.vcxproj",
            "the walk points `MSBuildThisFile*` at the file it enters whether or not it was seeded"
        );
    }
}
