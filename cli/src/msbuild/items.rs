//! MSBuild's second evaluation pass: settling deferred item groups against final properties.
//!
//! Pass 1 (`imports.rs`) hands over every `ItemGroup` and `ItemDefinitionGroup` in evaluation order
//! with its `Condition` **unevaluated**, because MSBuild decides which of them apply against the
//! property values the whole graph *ended* with rather than the ones in scope where each element was
//! written. This is where those conditions are finally evaluated, and where the `ClCompile` items
//! that become compilation database entries are collected.
//!
//! Order is load-bearing twice over. An `ItemDefinitionGroup` applies only to items declared
//! **after** it, so the defaults are accumulated during the walk and snapshotted into each item as
//! it is declared rather than gathered up front — which is what ADR 10's spliced-in-order stream
//! exists to make possible. See [ADR 12](../../../docs/adr/12-clcompile-flag-extraction.md).

use std::path::{Path, PathBuf};

use crate::msbuild::eval::{self, Metadata, Properties};
use crate::msbuild::imports::ProjectTree;
use crate::msbuild::paths;
use crate::msbuild::project::{Element, Item, Setting};
use crate::msbuild::reserved;
use crate::{ConfigPlatform, Reporter, Result};

/// One `ClCompile` item, with the metadata that applies to it, everything expanded.
#[derive(Debug, Clone)]
pub struct SourceItem {
    /// Absolute and normalized
    pub path: PathBuf,
    pub metadata: Metadata,
    /// The file that declared it, and the line. Diagnostics only.
    pub declared_in: PathBuf,
    pub line: usize,
}

/// What one deferred element is settled against: the directory its item paths resolve to, the
/// property values to expand with, and the file to name in a diagnostic.
///
/// Grouped because the three always travel together and only `file` varies between elements — and
/// because threading them individually pushed `include_item` past what clippy will accept.
struct Scope<'a> {
    /// The `.vcxproj`'s directory. Constant for a whole `settle`, per the note below.
    dir: &'a Path,
    /// The final bag, with `MSBuildThisFile*` pointed back at `file`.
    props: &'a Properties,
    /// The file that wrote the element. Diagnostics only; paths do not resolve against it.
    file: &'a Path,
}

/// Settle `tree.deferred` into the source files the project compiles.
///
/// `project` is the `.vcxproj` itself, because a relative item path resolves against **its**
/// directory even when the item is written in an imported `.props`. This is because MSBuild
/// evaluates with the project as its current directory, which is why a shared sheet has to use
/// `$(MSBuildThisFileDirectory)` to reach its siblings.
///
/// The bag is *cloned* rather than borrowed because of that same family. Pass 1 restores
/// `MSBuildThisFile*` on the way back out of every import, so the values the graph ended with point
/// at the root project — settling a sheet's deferred elements against them as they stand would
/// resolve its `$(MSBuildThisFileDirectory)include` to a directory beside the `.vcxproj` that
/// nothing created. Each element is therefore evaluated with that family pointed back at the file
/// that wrote it, using the same two calls `imports::Walk::enter` brackets a file with. Everything
/// else in the bag stays as evaluation left it, which is what MSBuild specifies.
pub fn settle(
    tree: &ProjectTree,
    project: &Path,
    reporter: &mut Reporter,
) -> Result<Vec<SourceItem>> {
    let dir = project.parent().unwrap_or(Path::new(""));
    let mut properties = tree.properties.clone();
    let mut defaults = Metadata::new();
    let mut sources: Vec<SourceItem> = Vec::new();

    for deferred in &tree.deferred {
        let file = deferred.file.as_path();
        let outer = reserved::enter_file(&mut properties, file);
        let scope = Scope {
            dir,
            props: &properties,
            file,
        };

        match &deferred.element {
            Element::ItemDefinitions(group) => {
                if holds(group.condition.as_deref(), &scope, group.line, reporter)? {
                    for definition in &group.definitions {
                        if !definition.item_type.eq_ignore_ascii_case("ClCompile") {
                            continue;
                        }
                        if !holds(
                            definition.condition.as_deref(),
                            &scope,
                            definition.line,
                            reporter,
                        )? {
                            continue;
                        }

                        apply(&mut defaults, &definition.metadata, &scope, reporter)?;
                    }
                }
            }

            Element::Items(group) => {
                if holds(group.condition.as_deref(), &scope, group.line, reporter)? {
                    for item in &group.items {
                        if !item.item_type.eq_ignore_ascii_case("ClCompile") {
                            continue;
                        }

                        if !holds(item.condition.as_deref(), &scope, item.line, reporter)? {
                            continue;
                        }

                        // Include, Update, and Remove are mutually exclusive in practice, and taken
                        // in that order so an item spelling two only has the first acted on
                        if let Some(include) = &item.include {
                            include_item(&mut sources, item, include, &defaults, &scope, reporter)?;
                        } else if let Some(update) = &item.update {
                            update_item(&mut sources, item, update, &scope, reporter)?;
                        } else if let Some(remove) = &item.remove {
                            let dropped =
                                specs(remove, &scope, item.line, Wildcard::Harmless, reporter)?;
                            sources.retain(|source| !dropped.contains(&source.path));
                        }
                    }
                }
            }

            // Pass 1 defers nothing else
            _ => {}
        }

        reserved::restore(&mut properties, outer);
    }
    Ok(sources)
}

/// Add every file an `Include` names, then subtract what its `Exclude` names.
///
/// Each new item takes a **clone** of the defaults as they stand now, which is the
/// applies-forward-only rule. Its own metadata then goes on top, so `%(X)` in a per-file override
/// sees the project default and can extend it rather than replace it.
fn include_item(
    sources: &mut Vec<SourceItem>,
    item: &Item,
    include: &str,
    defaults: &Metadata,
    scope: &Scope,
    reporter: &mut Reporter,
) -> Result<()> {
    let included = specs(include, scope, item.line, Wildcard::Loses, reporter)?;

    // What an unexpanded `Exclude` wildcard costs depends on what the `Include` put within its
    // reach, so the reading has to be decided here rather than at the `Exclude` itself. Paired with
    // a wildcard `Include` — the usual case — nothing was added and there was nothing to subtract.
    // Paired with a literal one there are real files it would have removed and did not.
    let exclusion = if included.is_empty() {
        Wildcard::Harmless
    } else {
        Wildcard::Keeps
    };

    for path in included {
        if sources.iter().any(|seen| seen.path == path) {
            // MSBuild would compile it twice. The compilation database can't really deal with that
            // and would just drop the second entry anyway
            reporter.warn_at(
                scope.file,
                Some(item.line),
                format!(
                    "`{}` is already a source file of this project. Ignoring second declaration.",
                    path.display()
                ),
            );
            continue;
        }

        let mut source = SourceItem {
            path,
            metadata: defaults.clone(),
            declared_in: scope.file.to_path_buf(),
            line: item.line,
        };
        apply(&mut source.metadata, &item.metadata, scope, reporter)?;
        sources.push(source);
    }

    // `Exclude` mostly earns its keep against a wildcard, which is not expanded — so in practice it
    // usually subtracts nothing. Honoured anyway as a literal `Exclude` is cheap and easy to obey.
    if let Some(exclude) = &item.exclude {
        let dropped = specs(exclude, scope, item.line, exclusion, reporter)?;
        sources.retain(|source| !dropped.contains(&source.path));
    }

    Ok(())
}

/// Attach metadata to items already collected. How a shared sheet reaches a specific file.
fn update_item(
    sources: &mut [SourceItem],
    item: &Item,
    update: &str,
    scope: &Scope,
    reporter: &mut Reporter,
) -> Result<()> {
    for path in specs(update, scope, item.line, Wildcard::Loses, reporter)? {
        let mut matched = false;

        for source in sources.iter_mut().filter(|source| source.path == path) {
            apply(&mut source.metadata, &item.metadata, scope, reporter)?;
            matched = true;
        }

        if !matched {
            // An `Update` naming nothing is a typo or bad path. Its settings vanish silently under
            // msbuild. Worth floating up!
            reporter.warn_at(
                scope.file,
                Some(item.line),
                format!(
                    "`<ClCompile Update=\"{}\">` names a file this project does not compile",
                    path.display()
                ),
            );
        }
    }

    Ok(())
}

// The configuration/platform pairs that the project declares, from its `ProjectConfiguration` items.
//
// Group configurations are deliberately **not** evaluated here. This merely asks what the file
// offers, VS writes that group unconditionally and `settle` already reports any findings those
// conditions produce.
pub fn declared_configurations(
    tree: &ProjectTree,
    reporter: &mut Reporter,
) -> Result<Vec<ConfigPlatform>> {
    let mut declared = Vec::new();

    for deferred in &tree.deferred {
        let Element::Items(group) = &deferred.element else {
            continue;
        };

        for item in &group.items {
            if !item.item_type.eq_ignore_ascii_case("ProjectConfiguration") {
                continue;
            }

            let Some(include) = &item.include else {
                continue;
            };
            let value = eval::expand(include, &tree.properties)?.report(
                reporter,
                &deferred.file,
                item.line,
            );

            if let Some((configuration, platform)) = value.split_once('|') {
                declared.push(ConfigPlatform::new(configuration.trim(), platform.trim()));
            }
        }
    }

    Ok(declared)
}

/// What an unexpanded wildcard costs, which is what separates the three wordings.
///
/// The two costly cases point in opposite directions and cannot share a sentence: an `Include` that
/// does not expand leaves files *out* of the database, while an `Exclude` that does not expand
/// leaves them *in*. Which of the two an `Exclude` is depends on its `Include` — see
/// [`include_item`].
#[derive(Clone, Copy)]
enum Wildcard {
    /// In an `Include` or an `Update`: files, or their settings, are missing.
    Loses,
    /// In an `Exclude` or a `Remove` that had something to subtract from: files the build does not
    /// compile stay in the database, so clangd indexes translation units the build never sees.
    Keeps,
    /// In an `Exclude` or a `Remove` with nothing in reach, which is the usual pairing — the
    /// `Include` wildcard was not expanded either, so nothing was added to subtract from.
    Harmless,
}

/// Expand an `Include`/`Update`/`Remove`/`Exclude` attribute into absolute paths.
///
/// The attribute is a `;` delimited list. Wildcards are not expanded, but are warned.
fn specs(
    raw: &str,
    scope: &Scope,
    line: usize,
    wildcard: Wildcard,
    reporter: &mut Reporter,
) -> Result<Vec<PathBuf>> {
    let value = eval::expand(raw, scope.props)?.report(reporter, scope.file, line);

    let mut out = Vec::new();

    for spec in value
        .split(';')
        .map(str::trim)
        .filter(|spec| !spec.is_empty())
    {
        if spec.contains('*') || spec.contains('?') {
            match wildcard {
                Wildcard::Loses => reporter.warn_at(
                    scope.file,
                    Some(line),
                    format!(
                        "`{spec}` is a wildcard, which is not expanded. \
                         What it names will be missing from the compilation database."
                    ),
                ),
                Wildcard::Keeps => reporter.warn_at(
                    scope.file,
                    Some(line),
                    format!(
                        "`{spec}` is a wildcard, which is not expanded. \
                         What it names was not removed from the compilation database."
                    ),
                ),
                Wildcard::Harmless => reporter.note_at(
                    scope.file,
                    Some(line),
                    format!("`{spec}` is a wildcard, which is not expanded. Nothing was removed."),
                ),
            }
            continue;
        }

        out.push(paths::resolve(scope.dir, spec));
    }

    Ok(out)
}

/// Assign a run of metadata settings, in document order.
///
/// Order matters only within `%(X)`. A later sibling sees what an earlier assigned.
fn apply(
    metadata: &mut Metadata,
    settings: &[Setting],
    scope: &Scope,
    reporter: &mut Reporter,
) -> Result<()> {
    for setting in settings {
        if !holds(setting.condition.as_deref(), scope, setting.line, reporter)? {
            continue;
        }

        metadata
            .assign(&setting.name, &setting.value, scope.props)?
            .report(reporter, scope.file, setting.line);
    }

    Ok(())
}

/// Evaluate a `Condition` against the properties collected in the first pass.
fn holds(
    condition: Option<&str>,
    scope: &Scope,
    line: usize,
    reporter: &mut Reporter,
) -> Result<bool> {
    let Some(condition) = condition else {
        return Ok(true);
    };

    Ok(
        eval::condition_holds(condition, scope.props, scope.file, line)?
            .report(reporter, scope.file, line),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msbuild::eval::Properties;
    use crate::msbuild::imports::{Deferred, ProjectTree};
    use crate::msbuild::project::{
        Element, Item, ItemDefinition, ItemDefinitionGroup, ItemGroup, Setting,
    };
    use crate::{ConfigPlatform, ErrorKind, Reporter, Severity};
    use std::path::PathBuf;

    /// An absolute path spelled the way the host platform spells one.
    ///
    /// The same construction `reserved.rs` and `paths.rs` use: a `D:\` literal is *relative* on the
    /// Linux CI runner, and these tests compare resolved absolute paths.
    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    /// The project every case below settles. Never read from disk — `settle` does no I/O.
    fn project() -> PathBuf {
        absolute(&["code", "app", "App.vcxproj"])
    }

    /// A property sheet in a *different* directory to the project, for the cases about which
    /// directory a relative item path resolves against.
    fn sheet() -> PathBuf {
        absolute(&["code", "shared", "common.props"])
    }

    fn bag(pairs: &[(&str, &str)]) -> Properties {
        let mut properties = Properties::new();
        for (name, value) in pairs {
            properties.set(name, *value);
        }
        properties
    }

    /// A tree carrying `deferred`, as pass 1 would hand it over.
    ///
    /// `files` is left empty: nothing in this pass reads it, and populating it would imply these
    /// elements came from files that exist.
    fn tree(properties: Properties, deferred: Vec<Deferred>) -> ProjectTree {
        ProjectTree {
            files: Vec::new(),
            deferred,
            properties,
        }
    }

    /// A tree whose elements all came from the project file itself, which is the ordinary case.
    fn from_project(elements: Vec<Element>) -> ProjectTree {
        from_project_with(Properties::new(), elements)
    }

    fn from_project_with(properties: Properties, elements: Vec<Element>) -> ProjectTree {
        let deferred = elements
            .into_iter()
            .map(|element| Deferred {
                file: project(),
                element,
            })
            .collect();
        tree(properties, deferred)
    }

    fn setting(name: &str, value: &str) -> Setting {
        Setting {
            name: name.to_string(),
            value: value.to_string(),
            condition: None,
            line: 4,
        }
    }

    /// An `<ItemDefinitionGroup><ClCompile>` carrying `metadata` — where nearly every flag lives.
    fn defaults(metadata: &[(&str, &str)]) -> Element {
        guarded_defaults(None, metadata)
    }

    fn guarded_defaults(condition: Option<&str>, metadata: &[(&str, &str)]) -> Element {
        Element::ItemDefinitions(ItemDefinitionGroup {
            condition: condition.map(str::to_string),
            definitions: vec![ItemDefinition {
                item_type: "ClCompile".to_string(),
                condition: None,
                metadata: metadata.iter().map(|(n, v)| setting(n, v)).collect(),
                line: 3,
            }],
            line: 2,
        })
    }

    /// An `<ItemGroup>` holding `items`.
    fn group(items: Vec<Item>) -> Element {
        guarded_group(None, items)
    }

    fn guarded_group(condition: Option<&str>, items: Vec<Item>) -> Element {
        Element::Items(ItemGroup {
            condition: condition.map(str::to_string),
            items,
            line: 10,
        })
    }

    /// An item declaring nothing, for the builders below to fill in.
    fn item(item_type: &str) -> Item {
        Item {
            item_type: item_type.to_string(),
            include: None,
            exclude: None,
            remove: None,
            update: None,
            condition: None,
            metadata: Vec::new(),
            line: 11,
        }
    }

    fn include(spec: &str) -> Item {
        Item {
            include: Some(spec.to_string()),
            ..item("ClCompile")
        }
    }

    fn include_with(spec: &str, metadata: &[(&str, &str)]) -> Item {
        Item {
            metadata: metadata.iter().map(|(n, v)| setting(n, v)).collect(),
            ..include(spec)
        }
    }

    /// Settle with a quiet reporter, asserting nothing was warned about — what most cases want.
    fn settled(tree: &ProjectTree) -> Vec<SourceItem> {
        let mut reporter = Reporter::new(false);
        let sources = settle(tree, &project(), &mut reporter).expect("settles");
        assert!(
            reporter.warnings().is_empty(),
            "unexpected warnings: {:?}",
            reporter.warnings()
        );
        sources
    }

    /// Settle and hand back the reporter, for the cases asserting on what was warned about.
    fn settled_reporting(tree: &ProjectTree) -> (Vec<SourceItem>, Reporter) {
        let mut reporter = Reporter::new(false);
        let sources = settle(tree, &project(), &mut reporter).expect("settles");
        (sources, reporter)
    }

    /// The source file names, so assertions read the same on either platform.
    fn names(sources: &[SourceItem]) -> Vec<String> {
        sources
            .iter()
            .map(|source| {
                source
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// One item's value for `name`, which is what the metadata cases assert on.
    fn value<'a>(source: &'a SourceItem, name: &str) -> &'a str {
        source
            .metadata
            .get(name)
            .unwrap_or_else(|| panic!("%({name}) was never assigned"))
    }

    /// The only source file, for the many cases that declare exactly one.
    fn only(sources: &[SourceItem]) -> &SourceItem {
        match sources {
            [source] => source,
            other => panic!("expected exactly one source file, got {}", other.len()),
        }
    }

    #[test]
    fn a_project_default_reaches_every_source_file() {
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "WIN32;NDEBUG")]),
            group(vec![include("main.cpp"), include("util.cpp")]),
        ]);

        let sources = settled(&tree);

        assert_eq!(names(&sources), ["main.cpp", "util.cpp"]);
        for source in &sources {
            assert_eq!(value(source, "PreprocessorDefinitions"), "WIN32;NDEBUG");
        }
    }

    #[test]
    fn a_file_level_setting_overrides_the_project_default() {
        // The per-file override Visual Studio writes when you change one file's properties. It
        // replaces rather than merges, because the file said nothing about inheriting.
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "WIN32")]),
            group(vec![
                include("main.cpp"),
                include_with("special.cpp", &[("PreprocessorDefinitions", "SPECIAL")]),
            ]),
        ]);

        let sources = settled(&tree);

        assert_eq!(value(&sources[0], "PreprocessorDefinitions"), "WIN32");
        assert_eq!(value(&sources[1], "PreprocessorDefinitions"), "SPECIAL");
        assert_eq!(
            value(&sources[0], "PreprocessorDefinitions"),
            "WIN32",
            "one file's override must not reach back into a sibling's snapshot"
        );
    }

    #[test]
    fn a_file_level_setting_can_extend_the_project_default() {
        // `%(...)` in a per-file override resolves against the item-definition value, which is what
        // makes "inherit and add one define" expressible at all. This is the composition MVP-12's
        // issue calls `%(Inherited)`; the file spells it as a self-reference.
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "WIN32")]),
            group(vec![include_with(
                "special.cpp",
                &[(
                    "PreprocessorDefinitions",
                    "%(PreprocessorDefinitions);SPECIAL",
                )],
            )]),
        ]);

        let sources = settled(&tree);

        assert_eq!(
            value(only(&sources), "PreprocessorDefinitions"),
            "WIN32;SPECIAL"
        );
    }

    #[test]
    fn inheritance_composes_across_two_item_definition_groups() {
        // A property sheet sets the shared list and the project adds to it. Both groups arrive in
        // the same spliced stream, so composition depends only on their order in it.
        let tree = from_project(vec![
            defaults(&[("AdditionalIncludeDirectories", "shared")]),
            defaults(&[(
                "AdditionalIncludeDirectories",
                "%(AdditionalIncludeDirectories);local",
            )]),
            group(vec![include("main.cpp")]),
        ]);

        let sources = settled(&tree);

        assert_eq!(
            value(only(&sources), "AdditionalIncludeDirectories"),
            "shared;local"
        );
    }

    #[test]
    fn an_item_definition_group_after_an_item_does_not_affect_it() {
        // MSBuild's rule, and the reason the defaults are snapshotted per item rather than gathered
        // first. Visual Studio always writes the definitions above the items, so a project relying
        // on this is unusual — but gathering up front would silently apply settings to files that
        // MSBuild itself compiles without them.
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "EARLY")]),
            group(vec![include("early.cpp")]),
            defaults(&[("PreprocessorDefinitions", "LATE")]),
            group(vec![include("late.cpp")]),
        ]);

        let sources = settled(&tree);

        assert_eq!(names(&sources), ["early.cpp", "late.cpp"]);
        assert_eq!(value(&sources[0], "PreprocessorDefinitions"), "EARLY");
        assert_eq!(value(&sources[1], "PreprocessorDefinitions"), "LATE");
    }

    #[test]
    fn a_false_group_condition_drops_its_definitions() {
        // The configuration-gated `ItemDefinitionGroup` every Visual Studio project writes twice
        // over. Applying the wrong one is how a Debug database ends up with Release flags.
        let properties = bag(&[("Configuration", "Debug"), ("Platform", "x64")]);
        let tree = from_project_with(
            properties,
            vec![
                guarded_defaults(
                    Some("'$(Configuration)|$(Platform)'=='Debug|x64'"),
                    &[("PreprocessorDefinitions", "_DEBUG")],
                ),
                guarded_defaults(
                    Some("'$(Configuration)|$(Platform)'=='Release|x64'"),
                    &[("PreprocessorDefinitions", "NDEBUG")],
                ),
                group(vec![include("main.cpp")]),
            ],
        );

        let sources = settled(&tree);

        assert_eq!(value(only(&sources), "PreprocessorDefinitions"), "_DEBUG");
    }

    #[test]
    fn a_false_group_condition_drops_its_items() {
        let properties = bag(&[("Platform", "x64")]);
        let tree = from_project_with(
            properties,
            vec![
                guarded_group(Some("'$(Platform)'=='x64'"), vec![include("wide.cpp")]),
                guarded_group(Some("'$(Platform)'=='Win32'"), vec![include("narrow.cpp")]),
            ],
        );

        assert_eq!(names(&settled(&tree)), ["wide.cpp"]);
    }

    #[test]
    fn a_false_item_condition_drops_that_item_alone() {
        let properties = bag(&[("BuildLegacy", "false")]);
        let tree = from_project_with(
            properties,
            vec![group(vec![
                include("main.cpp"),
                Item {
                    condition: Some("'$(BuildLegacy)'=='true'".to_string()),
                    ..include("legacy.cpp")
                },
            ])],
        );

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn a_false_metadata_condition_leaves_the_inherited_value_alone() {
        let properties = bag(&[("UseUnicode", "false")]);
        let tree = from_project_with(
            properties,
            vec![
                defaults(&[("PreprocessorDefinitions", "WIN32")]),
                Element::ItemDefinitions(ItemDefinitionGroup {
                    condition: None,
                    definitions: vec![ItemDefinition {
                        item_type: "ClCompile".to_string(),
                        condition: None,
                        metadata: vec![Setting {
                            condition: Some("'$(UseUnicode)'=='true'".to_string()),
                            ..setting("PreprocessorDefinitions", "UNICODE")
                        }],
                        line: 3,
                    }],
                    line: 2,
                }),
                group(vec![include("main.cpp")]),
            ],
        );

        assert_eq!(
            value(only(&settled(&tree)), "PreprocessorDefinitions"),
            "WIN32"
        );
    }

    #[test]
    fn conditions_are_evaluated_against_the_property_values_the_graph_ended_with() {
        // The premise of the whole pass, and why MVP-10 deferred these elements rather than
        // evaluating them where it found them: `$(Late)` is assigned by a `.props` imported *after*
        // the item group was read, and MSBuild still applies the group. Evaluating in place would
        // have compared against an empty value and dropped it.
        let properties = bag(&[("Late", "yes")]);
        let tree = from_project_with(
            properties,
            vec![guarded_group(
                Some("'$(Late)'=='yes'"),
                vec![include("main.cpp")],
            )],
        );

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn a_macro_in_an_include_is_expanded() {
        let properties = bag(&[("Generated", "gen")]);
        let tree = from_project_with(
            properties,
            vec![group(vec![include(r"$(Generated)\parser.cpp")])],
        );

        assert_eq!(
            only(&settled(&tree)).path,
            absolute(&["code", "app", "gen", "parser.cpp"])
        );
    }

    #[test]
    fn a_semicolon_list_include_declares_every_file() {
        // Legal MSBuild that Visual Studio never writes, and cheap to honour: splitting is the same
        // work the metadata lists need anyway. Empty entries from `a.cpp;;b.cpp` are dropped.
        let tree = from_project(vec![group(vec![include("main.cpp;;util.cpp")])]);

        assert_eq!(names(&settled(&tree)), ["main.cpp", "util.cpp"]);
    }

    #[test]
    fn a_relative_path_resolves_against_the_project_directory() {
        // MSBuild evaluates with the *project* as the current directory, so an item written in a
        // shared property sheet resolves against the `.vcxproj`, not against the sheet — which is
        // exactly why a sheet that means its own directory has to spell
        // `$(MSBuildThisFileDirectory)`. Resolving against the declaring file instead would point
        // clangd at a source file that does not exist.
        let tree = tree(
            Properties::new(),
            vec![Deferred {
                file: sheet(),
                element: group(vec![include("main.cpp")]),
            }],
        );

        assert_eq!(
            only(&settled(&tree)).path,
            absolute(&["code", "app", "main.cpp"]),
            "resolved against the project, not against {}",
            sheet().display()
        );
    }

    #[test]
    fn a_declaring_file_is_recorded_for_diagnostics() {
        // The path resolves against the project; the *diagnostic* still has to name the file the
        // user would open to fix it.
        let tree = tree(
            Properties::new(),
            vec![Deferred {
                file: sheet(),
                element: group(vec![include("main.cpp")]),
            }],
        );

        let source = only(&settled(&tree)).clone();
        assert_eq!(source.declared_in, sheet());
        assert_eq!(source.line, 11);
    }

    #[test]
    fn an_absolute_include_is_kept_as_written() {
        let elsewhere = absolute(&["vendor", "src", "third_party.cpp"]);
        let tree = from_project(vec![group(vec![include(&elsewhere.to_string_lossy())])]);

        assert_eq!(only(&settled(&tree)).path, elsewhere);
    }

    #[test]
    fn a_parent_relative_include_is_normalized() {
        let tree = from_project(vec![group(vec![include(r"..\shared\util.cpp")])]);

        assert_eq!(
            only(&settled(&tree)).path,
            absolute(&["code", "shared", "util.cpp"])
        );
    }

    #[test]
    fn a_wildcard_warns_and_contributes_nothing() {
        // ADR 12's decision, and the point of it. `<ClCompile Include="src\*.cpp" />` is legal
        // MSBuild; before this the whole project came out with no source files and no explanation,
        // which is the worst available outcome. MVP-37 owns actually expanding it.
        let tree = from_project(vec![group(vec![
            include(r"src\*.cpp"),
            include("main.cpp"),
        ])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert_eq!(
            names(&sources),
            ["main.cpp"],
            "the wildcard contributes nothing and does not take its siblings with it"
        );

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert!(
            warning.message.contains(r"src\*.cpp"),
            "names the spec that was skipped: {}",
            warning.message
        );
        assert_eq!(warning.line, Some(11));
        assert_eq!(warning.file.as_deref(), Some(project().as_path()));
    }

    #[test]
    fn a_question_mark_is_a_wildcard_too() {
        let tree = from_project(vec![group(vec![include("test?.cpp")])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert!(sources.is_empty());
        assert_eq!(reporter.warnings().len(), 1);
    }

    /// The same construct means two different things, which MVP-13 splits by severity.
    ///
    /// In an `Include` an unexpanded wildcard costs source files, so it is a warning. In a
    /// `Remove` or an `Exclude` it subtracts from a list nothing was added to — because the
    /// matching `Include` wildcard was not expanded either — so nothing is lost and warning about
    /// it was a false positive. See ADR 13.
    #[test]
    fn a_wildcard_include_warns_but_a_wildcard_remove_only_notes() {
        let tree = from_project(vec![
            group(vec![include(r"src\*.cpp"), include("main.cpp")]),
            group(vec![Item {
                remove: Some(r"src\*_test.cpp".to_string()),
                ..item("ClCompile")
            }]),
        ]);

        let (sources, reporter) = settled_reporting(&tree);

        assert_eq!(names(&sources), ["main.cpp"]);

        let severities: Vec<Severity> = reporter
            .warnings()
            .iter()
            .map(|finding| finding.severity)
            .collect();
        assert_eq!(
            severities,
            [Severity::Warning, Severity::Note],
            "the `Include` loses files; the `Remove` loses nothing: {:?}",
            reporter.warnings()
        );
    }

    /// The usual pairing, and the case the `Harmless` reading was written for: both halves are
    /// wildcards, so the `Include` added nothing and the `Exclude` had nothing to subtract from.
    /// Only the `Include` is worth a warning.
    #[test]
    fn a_wildcard_exclude_only_notes_when_the_include_added_nothing() {
        let tree = from_project(vec![group(vec![Item {
            exclude: Some(r"*.generated.cpp".to_string()),
            ..include(r"src\*.cpp")
        }])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert!(sources.is_empty());

        let severities: Vec<Severity> = reporter
            .warnings()
            .iter()
            .map(|finding| finding.severity)
            .collect();
        assert_eq!(
            severities,
            [Severity::Warning, Severity::Note],
            "{:?}",
            reporter.warnings()
        );
    }

    /// The case the unconditional `Harmless` reading got wrong. A literal `Include` puts real files
    /// within the `Exclude`'s reach, and not expanding it leaves them in the database — so clangd
    /// is handed a translation unit the build does not compile. Nothing is *missing*, which is why
    /// this needs a wording of its own rather than the `Include` one pointed backwards.
    #[test]
    fn a_wildcard_exclude_warns_when_the_include_was_literal() {
        let tree = from_project(vec![group(vec![Item {
            exclude: Some(r"*.generated.cpp".to_string()),
            ..include("main.cpp;foo.generated.cpp")
        }])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert_eq!(
            names(&sources),
            ["main.cpp", "foo.generated.cpp"],
            "the file the `Exclude` names is still there, which is the problem being reported"
        );

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert!(
            warning.message.contains("not removed"),
            "says which direction it went wrong in: {}",
            warning.message
        );
    }

    /// The boundary between the two readings is whether the `Include` produced anything, not
    /// whether it was *written* as a wildcard — a literal spec that expands to nothing through
    /// `$(Undefined)` leaves the `Exclude` with nothing in reach just the same.
    #[test]
    fn an_include_that_expands_to_nothing_leaves_the_exclude_harmless() {
        let tree = from_project(vec![group(vec![Item {
            exclude: Some(r"*.generated.cpp".to_string()),
            ..include("$(Missing)")
        }])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert!(sources.is_empty());
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|finding| finding.severity == Severity::Note),
            "the exclude is harmless: {:?}",
            reporter.warnings()
        );
    }

    /// A wildcard in an `Update` loses per-file settings rather than files, which is still the
    /// flags being wrong — so it stays a warning alongside `Include`.
    #[test]
    fn a_wildcard_update_warns() {
        let tree = from_project(vec![
            group(vec![include("main.cpp")]),
            group(vec![Item {
                update: Some(r"src\*.cpp".to_string()),
                metadata: vec![setting("PreprocessorDefinitions", "UPDATED")],
                ..item("ClCompile")
            }]),
        ]);

        let (_, reporter) = settled_reporting(&tree);

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
    }

    #[test]
    fn a_duplicate_declaration_warns_and_keeps_the_first() {
        // MSBuild would compile the file twice. A compilation database cannot say that — clangd
        // reads the first entry it finds — so the second is dropped rather than left to shadow the
        // settings of the first depending on which entry a consumer happens to read.
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "FIRST")]),
            group(vec![include("main.cpp")]),
            defaults(&[("PreprocessorDefinitions", "SECOND")]),
            group(vec![include("main.cpp")]),
        ]);

        let (sources, reporter) = settled_reporting(&tree);

        assert_eq!(names(&sources), ["main.cpp"]);
        assert_eq!(
            value(only(&sources), "PreprocessorDefinitions"),
            "FIRST",
            "the surviving entry keeps the settings it was declared with"
        );
        assert_eq!(reporter.warnings().len(), 1);
        assert!(
            reporter.warnings()[0].message.contains("main.cpp"),
            "names the file: {}",
            reporter.warnings()[0].message
        );
        // A warning rather than a note: the dropped declaration carried `SECOND`, and settings
        // that were asked for and did not arrive are the case this channel exists for.
        assert_eq!(reporter.warnings()[0].severity, Severity::Warning);
    }

    #[test]
    fn remove_drops_a_file_declared_earlier() {
        let tree = from_project(vec![
            group(vec![include("main.cpp"), include("legacy.cpp")]),
            group(vec![Item {
                remove: Some("legacy.cpp".to_string()),
                ..item("ClCompile")
            }]),
        ]);

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn exclude_drops_a_file_the_same_item_declared() {
        // `Exclude` only earns its keep against a wildcard, which is not expanded — so in practice
        // this subtracts nothing. Honoured anyway: a literal `Exclude` is legal and free to obey.
        let tree = from_project(vec![group(vec![Item {
            exclude: Some("generated.cpp".to_string()),
            ..include("main.cpp;generated.cpp")
        }])]);

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn update_attaches_metadata_to_a_file_declared_earlier() {
        // How a `Directory.Build.props` or a shared sheet reaches one specific file, and the other
        // route per-file settings take into the database.
        let tree = from_project(vec![
            defaults(&[("PreprocessorDefinitions", "WIN32")]),
            group(vec![include("main.cpp"), include("util.cpp")]),
            group(vec![Item {
                update: Some("util.cpp".to_string()),
                metadata: vec![setting(
                    "PreprocessorDefinitions",
                    "%(PreprocessorDefinitions);UPDATED",
                )],
                ..item("ClCompile")
            }]),
        ]);

        let sources = settled(&tree);

        assert_eq!(value(&sources[0], "PreprocessorDefinitions"), "WIN32");
        assert_eq!(
            value(&sources[1], "PreprocessorDefinitions"),
            "WIN32;UPDATED",
            "`%(...)` in an Update resolves against the item's own settled value"
        );
    }

    #[test]
    fn update_naming_a_file_the_project_does_not_compile_warns() {
        // A stale or misspelled path. Its settings vanish silently under MSBuild, and the file the
        // author meant to configure ends up with the defaults — worth saying out loud.
        let tree = from_project(vec![
            group(vec![include("main.cpp")]),
            group(vec![Item {
                update: Some("typo.cpp".to_string()),
                metadata: vec![setting("PreprocessorDefinitions", "NEVER")],
                ..item("ClCompile")
            }]),
        ]);

        let (sources, reporter) = settled_reporting(&tree);

        assert_eq!(names(&sources), ["main.cpp"]);
        assert_eq!(reporter.warnings().len(), 1);
        assert!(
            reporter.warnings()[0].message.contains("typo.cpp"),
            "names the file: {}",
            reporter.warnings()[0].message
        );
    }

    #[test]
    fn only_clcompile_items_become_source_files() {
        // A header is recorded by MVP-07's reader and simply is not a translation unit. Nor is a
        // `ProjectConfiguration`, which shares the accessor and would otherwise turn into a source
        // file named `Debug|x64`.
        let tree = from_project(vec![group(vec![
            include("main.cpp"),
            Item {
                include: Some("app.h".to_string()),
                ..item("ClInclude")
            },
            Item {
                include: Some("Debug|x64".to_string()),
                ..item("ProjectConfiguration")
            },
            Item {
                include: Some("app.rc".to_string()),
                ..item("ResourceCompile")
            },
        ])]);

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn an_item_type_is_matched_case_insensitively() {
        // MSBuild item names are case-insensitive and `<Clcompile>` builds in Visual Studio.
        // Treating it as a different type would drop the source file in silence.
        let tree = from_project(vec![
            Element::ItemDefinitions(ItemDefinitionGroup {
                condition: None,
                definitions: vec![ItemDefinition {
                    item_type: "clcompile".to_string(),
                    condition: None,
                    metadata: vec![setting("PreprocessorDefinitions", "WIN32")],
                    line: 3,
                }],
                line: 2,
            }),
            group(vec![Item {
                include: Some("main.cpp".to_string()),
                ..item("Clcompile")
            }]),
        ]);

        let sources = settled(&tree);

        assert_eq!(names(&sources), ["main.cpp"]);
        assert_eq!(
            value(only(&sources), "PreprocessorDefinitions"),
            "WIN32",
            "the definition group has to match the same way the item does"
        );
    }

    #[test]
    fn an_item_naming_nothing_is_skipped() {
        // MVP-07 already warned about it at parse time, so warning again here would double the
        // noise for one mistake.
        let tree = from_project(vec![group(vec![item("ClCompile"), include("main.cpp")])]);

        assert_eq!(names(&settled(&tree)), ["main.cpp"]);
    }

    #[test]
    fn an_empty_include_declares_nothing() {
        // `Include="$(Missing)"` against an unset property. The undefined reference is warned
        // about, but there is no path to add and no crash to be had.
        let tree = from_project(vec![group(vec![include("$(Missing)")])]);

        let (sources, reporter) = settled_reporting(&tree);

        assert!(sources.is_empty());
        assert!(
            reporter
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("Missing")),
            "the undefined reference is named: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn a_metadata_finding_names_the_file_and_line_it_came_from() {
        // MVP-08 and MVP-09 record findings without knowing where they came from; this pass is one
        // of the two places that holds both halves. MVP-13 owns the wording.
        let tree = tree(
            Properties::new(),
            vec![Deferred {
                file: sheet(),
                element: defaults(&[("AdditionalIncludeDirectories", "$(NoSuchProperty)inc")]),
            }],
        );

        let (_, reporter) = settled_reporting(&tree);

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.file.as_deref(), Some(sheet().as_path()));
        assert_eq!(warning.line, Some(4), "the setting's line, not the group's");
        assert!(
            warning.message.contains("NoSuchProperty"),
            "names the reference: {}",
            warning.message
        );
    }

    #[test]
    fn an_unsupported_condition_is_fatal() {
        // MVP-09's policy, inherited unchanged: inventing a boolean either drops an
        // `ItemDefinitionGroup` carrying every flag or applies one the file never asked for.
        // MVP-36 owns whether this should be downgraded for the toolchain's own files.
        let tree = from_project(vec![guarded_defaults(
            Some("$([MSBuild]::VersionGreaterThan('$(Platform)', '1'))"),
            &[("PreprocessorDefinitions", "WIN32")],
        )]);

        let error = settle(&tree, &project(), &mut Reporter::new(false))
            .expect_err("must not settle around a condition it cannot evaluate");

        assert_eq!(error.kind(), ErrorKind::UnsupportedConstruct);
    }

    #[test]
    fn a_tree_with_nothing_deferred_settles_to_nothing() {
        // A `.props` file evaluated on its own, or a project whose sources are all wildcards. Empty
        // is a legitimate answer here; `msbuild::evaluate` is what warns about it.
        let sources = settled(&tree(Properties::new(), Vec::new()));

        assert!(sources.is_empty());
    }

    #[test]
    fn declared_configurations_reads_the_project_configuration_items() {
        // The `Label="ProjectConfigurations"` group at the top of every `.vcxproj`. Read so that
        // asking for a pair the project never declares can be warned about, rather than surfacing
        // as a file list with no flags on it.
        let tree = from_project(vec![group(vec![
            Item {
                include: Some("Debug|Win32".to_string()),
                ..item("ProjectConfiguration")
            },
            Item {
                include: Some("Release|x64".to_string()),
                ..item("ProjectConfiguration")
            },
        ])]);

        let declared =
            declared_configurations(&tree, &mut Reporter::new(false)).expect("reads the items");

        assert_eq!(
            declared,
            [
                ConfigPlatform::new("Debug", "Win32"),
                ConfigPlatform::new("Release", "x64"),
            ]
        );
    }

    #[test]
    fn declared_configurations_ignores_an_item_that_is_not_a_pair() {
        // Hostile or hand-mangled input: an `Include` with no `|` in it names no pair. Skipped
        // rather than split into a configuration named after the whole string.
        let tree = from_project(vec![group(vec![
            Item {
                include: Some("nonsense".to_string()),
                ..item("ProjectConfiguration")
            },
            Item {
                include: Some("Debug|x64".to_string()),
                ..item("ProjectConfiguration")
            },
        ])]);

        let declared =
            declared_configurations(&tree, &mut Reporter::new(false)).expect("reads the items");

        assert_eq!(declared, [ConfigPlatform::new("Debug", "x64")]);
    }

    #[test]
    fn declared_configurations_is_empty_when_the_project_declares_none() {
        let tree = from_project(vec![group(vec![include("main.cpp")])]);

        let declared =
            declared_configurations(&tree, &mut Reporter::new(false)).expect("reads the items");

        assert!(
            declared.is_empty(),
            "a project declaring nothing must not look like one declaring the wrong thing"
        );
    }

    #[test]
    fn settle_does_not_read_the_filesystem() {
        // Deliberate: a `.vcxproj` may name a source file that has been deleted, and MSBuild fails
        // the *build* over that while clangd only needs the flags. Checking existence here would
        // also make every case above need a temp directory.
        let missing = absolute(&["definitely", "not", "here", "ghost.cpp"]);
        let tree = from_project(vec![group(vec![include(&missing.to_string_lossy())])]);

        assert_eq!(only(&settled(&tree)).path, missing);
    }
}
