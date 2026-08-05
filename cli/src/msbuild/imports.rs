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

use std::path::{Component, Path, PathBuf};

use crate::msbuild::eval::{self, Properties};
use crate::msbuild::project::{self, Element, Import, PropertyGroup};
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
/// `properties` is passed in, rather than built here so that MVP-11 can seed the reserved
/// properties without this function having to know what they are. An empty bag is also a valid
/// starting point. `$(VCTargetsPath)` and such would just expand to nothing until MVP-11 lands.
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
        let result = self.evaluate(path, referrer);
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
            self.reporter.warn_at(
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
            self.report(
                expansion.undefined,
                expansion.unsupported,
                file,
                property.line,
            );
        }

        Ok(())
    }

    fn import(&mut self, import: &Import, file: &Path) -> Result<()> {
        // The condition comes first on purpose. A false one is how sheet guards an optional import
        // and resolving the path first would mistakenly report a missing file.
        if !self.holds(import.condition.as_deref(), file, import.line)? {
            return Ok(());
        }

        let expansion = eval::expand(&import.project, &self.properties)?;
        self.report(
            expansion.undefined.clone(),
            expansion.unsupported.clone(),
            file,
            import.line,
        );

        let target = expansion.value.trim();
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
        self.enter(&normalize(&base.join(target)), Some(file))
    }

    /// Evaluate a `Condition`, reporting what it could not resolve. An absent condition still holds.
    fn holds(&mut self, condition: Option<&str>, file: &Path, line: usize) -> Result<bool> {
        let Some(condition) = condition else {
            return Ok(true);
        };

        let evaluation = eval::condition_holds(condition, &self.properties, file, line)?;
        self.report(evaluation.undefined, evaluation.unsupported, file, line);
        Ok(evaluation.holds)
    }

    /// Turns findings into diagnostic messages.
    fn report(
        &mut self,
        undefined: Vec<String>,
        unsupported: Vec<String>,
        file: &Path,
        line: usize,
    ) {
        for name in undefined {
            self.reporter.warn_at(
                file,
                Some(line),
                format!("`$({name})` is not defined and expanded to nothing"),
            );
        }

        for reference in unsupported {
            self.reporter.warn_at(
                file,
                Some(line),
                format!("`{reference}` is not supported and was left as written"),
            );
        }
    }
}

/// Make `path` absolute and resolve `.` and `..` lexically.
fn normalize(path: &Path) -> PathBuf {
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

/// Whether two normalized paths name the same file (case insensitive on Windows)
fn same_file(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        return left.as_os_str().eq_ignore_ascii_case(right.as_os_str());
    }

    left == right
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::ErrorKind;

    /// A throwaway directory tree, removed when the test that made it ends.
    ///
    /// Hand-rolled rather than added as a dev-dependency: `Cargo.toml` records the dependency list
    /// as settled, and this is thirty lines of `std::fs`. Import resolution reads real files and
    /// `fixtures/` is still empty — MVP-23 and MVP-24 own filling it, and an import cycle has no
    /// business in a corpus meant for manual clangd verification anyway.
    struct TempTree(PathBuf);

    impl TempTree {
        /// A fresh directory named for the calling test.
        ///
        /// The process id and a counter keep concurrent test binaries and parallel tests within
        /// one from colliding; the name makes a leftover directory traceable if a run is killed.
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);

            let path = std::env::temp_dir().join(format!(
                "msvclsp-{}-{name}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the temp directory is writable");
            TempTree(path)
        }

        /// Write a project file at `relative`, wrapping `body` in a `<Project>` element.
        ///
        /// `<Project>` is line 1, so **the body starts at line 2** — which is what the line-number
        /// assertions below count from. The namespace is omitted; MVP-07 made it optional.
        fn project(&self, relative: &str, body: &str) -> PathBuf {
            self.write(relative, &format!("<Project>\n{body}\n</Project>\n"))
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("the temp directory is writable");
            }
            std::fs::write(&path, contents).expect("the temp directory is writable");
            path
        }

        /// `relative` is spelled with `/`, which both platforms accept.
        fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
                file.strip_prefix(&dir.0)
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

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert!(
            warning.message.contains("already imported"),
            "says why it was skipped: {}",
            warning.message
        );
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
        // What every real `.vcxproj` does today, until MVP-11 and MVP-15 supply
        // `$(VCTargetsPath)`. Skipping beats resolving to the filesystem root.
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
}
