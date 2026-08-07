//! Lowering a `.vcxproj`/`.props`/`.targets` into an owned, document ordered IR.
//!
//! No evaluation happens here. Which `PropertyGroup` wins is decided by position in the file, ro
//! this reader's whole job is to preserve sequence and hand every `Condition` forward untouched.
//! MVP-08 will expand macros, MVP-09 will evaluate conditions, MVP-10 will follow imports.
//!
//! `roxmltree` borrows from the source text, so the IR is fully owned. One file is parsed, lowered,
//! and its `Doucment` dropped before the next import is read.

use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::{Error, Reporter, Result, progress};

/// How deeply XML elements may nest before a project file is refused, counting `<Project>` as level
/// 1. `roxmltree::Document::parse` recurses once per level and aborts the process rather than
/// return an error when the stack is gone, so depth has to be checked before parsing.
/// See [ADR 14](../../../docs/adr/14-xml-nesting-depth-guard.md). Real project typically reach 4 
/// levels of nesting. 8 times that is a generous buffer to account for truly complex projects.
const MAX_XML_DEPTH: usize = 32;

// One MSBuild project file, lowered into owned data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Absolute path to the file this came from.
    pub path: PathBuf,
    /// The children of `<Project>`, in document order.
    pub elements: Vec<Element>,
}

/// A top-level child of `<Project>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    Properties(PropertyGroup),
    ItemDefinitions(ItemDefinitionGroup),
    Items(ItemGroup),
    Import(Import),
    ImportGroup(ImportGroup),
    /// An element this reader models nothing for.
    /// Recorded rather than discarded so MVP-13 owns the message.
    Unsupported {
        name: String,
        line: usize,
    },
}

/// A `<Name Condition="...">value</Name>` child.
///
/// This one type covers two MSBuild concepts, property assignment and item metadata, because the
/// XML is identical and every later pass treats them identically: expand value and check the
/// condition. Splitting them buys nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: String,
    /// Raw and unexpanded --`$(ProjectDir)inc;%(AdditionalIncludeDirectories)` arrives as is
    pub value: String,
    pub condition: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyGroup {
    /// The group's own `Condition` gates all properties within it. Kept separate from each
    /// property's condition rather than combined into one string for simplicity
    pub condition: Option<String>,
    pub properties: Vec<Setting>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDefinitionGroup {
    pub condition: Option<String>,
    pub definitions: Vec<ItemDefinition>,
    pub line: usize,
}

/// Project-wide defaults for one item type -- the `<ClCompile>` inside an `ItemDefinitionGroup`,
/// which is where nearly every flag MVP-12 wants will probably live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDefinition {
    pub item_type: String,
    pub condition: Option<String>,
    pub metadata: Vec<Setting>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemGroup {
    pub condition: Option<String>,
    pub items: Vec<Item>,
    pub line: usize,
}

/// One item: a source file, a header, a `ProjectConfiguration` declaration
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub item_type: String,
    /// Raw and unexpanded, possibly a ';' list or a wildcard. Not resolved in this pass.
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub remove: Option<String>,
    ///`Update` attaches metadata to an item declared elsewhere, which is how per-file compiler
    ///settings reach MVP-12. Recorded for posterity.
    pub update: Option<String>,
    pub condition: Option<String>,
    pub metadata: Vec<Setting>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Raw and unexpanded. `$(SolutionDir)shared\common.props` resolves against the bag
    /// `msbuild/reserved.rs` seeds; `$(VCTargetsPath)\Microsoft.cpp.props` stays unresolved until
    /// MVP-15 supplies the Visual Studio paths.
    pub project: String,
    pub condition: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportGroup {
    pub condition: Option<String>,
    pub imports: Vec<Import>,
    pub line: usize,
}

impl ProjectFile {
    /// Every item of `item_type` across all `ItemGroup`s, in document order.
    ///
    /// No conditions are evaluated here, so these are *candidates*. MVP-09 will decide which groups
    /// and items apply. `ClCompile` is what the compilation database is built from. The same
    /// accessor reaches `ProjectConfiguration` and `ClInclude` for free.
    ///
    /// Matches case-insensitively
    pub fn items_of_type<'a>(&'a self, item_type: &'a str) -> impl Iterator<Item = &'a Item> {
        self.elements
            .iter()
            .filter_map(|element| match element {
                Element::Items(group) => Some(group),
                _ => None,
            })
            .flat_map(|group| &group.items)
            .filter(move |item| item.item_type.eq_ignore_ascii_case(item_type))
    }
}

/// Elements that are correct project content but cannot change how a source file compiles.
const IGNORED: [&str; 3] = ["Target", "UsingTask", "ProjectExtensions"];

/// Parse a project file.
///
/// A missing file ir an [`Error::Io`] rather than a dedicated variant: only MVP-10 knows which file
/// referenced this one, so `MissingImport` is reported there, by checking before calling in.
pub fn parse_project(path: &Path, reporter: &mut Reporter) -> Result<ProjectFile> {
    let text = std::fs::read_to_string(path).map_err(|err| Error::io(path, err))?;
    parse_project_text(&text, path, reporter)
}

/// Parse project XML text.
///
/// `path` names the file in errors and diagnostics. Nothing is read from the disk here
fn parse_project_text(text: &str, path: &Path, reporter: &mut Reporter) -> Result<ProjectFile> {
    // Before parsing, ensure we don't have a hostile XML.
    check_nesting_depth(text, path)?;

    let doc = Document::parse(text).map_err(|err| Error::ProjectParse {
        path: path.to_path_buf(),
        source: err,
    })?;

    let root = doc.root_element();
    if root.tag_name().name() != "Project" {
        return Err(Error::ProjectSchema {
            path: path.to_path_buf(),
            line: Some(line_of(&doc, root)),
            detail: format!(
                "root element is <{}>, expected <Project>",
                root.tag_name().name()
            ),
        });
    }

    let mut elements = Vec::new();

    for child in root.children().filter(Node::is_element) {
        let line = line_of(&doc, child);
        let condition = attribute(child, "Condition");

        // Matches on local name, so namespace is ignored.
        let element = match child.tag_name().name() {
            "PropertyGroup" => Element::Properties(PropertyGroup {
                condition,
                properties: settings(&doc, child, path, reporter),
                line,
            }),
            "ItemDefinitionGroup" => Element::ItemDefinitions(ItemDefinitionGroup {
                condition,
                definitions: item_definitions(&doc, child, path, reporter),
                line,
            }),
            "ItemGroup" => Element::Items(ItemGroup {
                condition,
                items: items(&doc, child, path, reporter),
                line,
            }),
            "Import" => Element::Import(import(&doc, child, path)?),
            "ImportGroup" => Element::ImportGroup(ImportGroup {
                condition,
                imports: imports(&doc, child, path, reporter)?,
                line,
            }),
            name if IGNORED.contains(&name) => continue,
            name => {
                reporter.warn_at(
                    path,
                    Some(line),
                    format!("`<{name}>` is not supported; whatever it configures will be ignored"),
                );
                Element::Unsupported {
                    name: name.to_string(),
                    line,
                }
            }
        };

        elements.push(element);
    }

    progress!(
        reporter,
        "{}: {} top-level element(s)",
        path.display(),
        elements.len()
    );

    Ok(ProjectFile {
        path: path.to_path_buf(),
        elements,
    })
}

/// The `<Name>value</Name>` children of a group, in document order.
///
/// Duplicates are retained. A `PropertyGroup` may assign the same name twice and the last one wins.
fn settings(doc: &Document, parent: Node, path: &Path, reporter: &mut Reporter) -> Vec<Setting> {
    parent
        .children()
        .filter(Node::is_element)
        .map(|child| {
            if child.children().any(|nested| nested.is_element()) {
                reporter.warn_at(
                    path,
                    Some(line_of(doc, child)),
                    format!(
                        "`<{}>` has element children, which are not valid in a value and were dropped. Only its text was read",
                        child.tag_name().name()
                        ));
            }
            Setting {
                name: child.tag_name().name().to_string(),
                value: text_of(child),
                condition: attribute(child, "Condition"),
                line: line_of(doc, child),
            }
        })
        .collect()
}

fn item_definitions(
    doc: &Document,
    parent: Node,
    path: &Path,
    reporter: &mut Reporter,
) -> Vec<ItemDefinition> {
    parent
        .children()
        .filter(Node::is_element)
        .map(|child| ItemDefinition {
            item_type: child.tag_name().name().to_string(),
            condition: attribute(child, "Condition"),
            metadata: settings(doc, child, path, reporter),
            line: line_of(doc, child),
        })
        .collect()
}

/// The items in an `ItemGroup`.
///
/// An item declaring none of `Include`/`Remove`/`Update` configures nothing. This triggers
/// a warning, but the item is still kept.
fn items(doc: &Document, parent: Node, path: &Path, reporter: &mut Reporter) -> Vec<Item> {
    parent
        .children()
        .filter(Node::is_element)
        .map(|child| {
            let item = Item {
                item_type: child.tag_name().name().to_string(),
                include: attribute(child, "Include"),
                exclude: attribute(child, "Exclude"),
                remove: attribute(child, "Remove"),
                update: attribute(child, "Update"),
                condition: attribute(child, "Condition"),
                metadata: settings(doc, child, path, reporter),
                line: line_of(doc, child),
            };

            if item.include.is_none() && item.remove.is_none() && item.update.is_none() {
                reporter.note_at(
                    path,
                    Some(item.line),
                    format!(
                        "`<{}>` names no Include, Remove or Update and so configures nothing",
                        item.item_type
                    ),
                );
            }
            item
        })
        .collect()
}

/// A single `<Import>`.
///
/// A missing `Project` attribute is fatal. An import is the mechanism that the whole flag set
/// arrives via, so one that can't be resolved means that the database will be missing settings with
/// no way to tell which.
fn import(doc: &Document, node: Node, path: &Path) -> Result<Import> {
    let line = line_of(doc, node);
    let Some(project) = node.attribute("Project") else {
        return Err(Error::ProjectSchema {
            path: path.to_path_buf(),
            line: Some(line),
            detail: "<Import> has no 'Project' attribute".to_string(),
        });
    };

    Ok(Import {
        project: project.to_string(),
        condition: attribute(node, "Condition"),
        line,
    })
}

/// The `<Import>` children of an `ImportGroup`. Anything else in one is warned about and skipped.
fn imports(
    doc: &Document,
    parent: Node,
    path: &Path,
    reporter: &mut Reporter,
) -> Result<Vec<Import>> {
    let mut imports = Vec::new();

    for child in parent.children().filter(Node::is_element) {
        if child.tag_name().name() == "Import" {
            imports.push(import(doc, child, path)?);
        } else {
            reporter.warn_at(
                path,
                Some(line_of(doc, child)),
                format!(
                    "`<{}>` inside an ImportGroup is not an Import and was skipped",
                    child.tag_name().name()
                ),
            );
        }
    }

    Ok(imports)
}

/// The 1-based line `node` starts on, for diagnostics and errors
fn line_of(doc: &Document, node: Node) -> usize {
    doc.text_pos_at(node.range().start).row as usize
}

/// An attribute's value, exactly as roxmltree decodes it.
///
/// Entity references are resolved and nothing else is normalized. A `Condition` reaches MVP-09
/// verbatim, including an empty one.
fn attribute(node: Node, name: &str) -> Option<String> {
    node.attribute(name).map(str::to_string)
}

/// The immediate text children of `node`, concatenated and trimmed.
///
/// Immediate rather than descendants, so walking only ever stays one level deep.
fn text_of(node: Node) -> String {
    let mut value = String::new();
    for child in node.children().filter(Node::is_text) {
        value.push_str(child.text().unwrap_or_default());
    }
    value.trim().to_string()
}

/// Check the nesting depth of document.
///
/// Returns an error if it exceeds `MAX_XML_DEPTH`. This is a byte scan rather than a parse as the
/// crash induced by a depth overrun happens within `Document::parse`. This is a guard, and not
/// a validator. Malformed XML is stepped over rather than reported, as that is the job of the
/// parser to detect. This just checks that the maximum depth does not exceed `MAX_XML_DEPTH` and
/// returns an [`Error::RecursionLimit`] if it does.
fn check_nesting_depth(text: &str, path: &Path) -> Result<()> {
    let bytes = text.as_bytes();
    let mut depth: usize = 0;
    let mut index = 0;

    while index < bytes.len() {
        let rest = &bytes[index..];

        if !rest.starts_with(b"<") {
            index += 1;
        } else if rest.starts_with(b"<!--") {
            // A Comment or CDATA may hold anything at all, including unbalanced tags. Skip to the
            // end of the comment
            index = skip_past(bytes, index + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            index = skip_past(bytes, index + 9, b"]]>");
        } else if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            // A DTD or processing instruction. Neither nests an element and `allow_dtd` is false so
            // a `<!DOCTYPE>` is rejected by the parser before it recurses.
            index = skip_past(bytes, index + 2, b">");
        } else if rest.starts_with(b"</") {
            // Saturating rather than checked. A close with no open is malformed and saying so
            // is the parsers job, not ours.
            depth = depth.saturating_sub(1);
            index = skip_past(bytes, index + 2, b">");
        } else if rest.get(1).is_some_and(|&byte| is_name_start(byte)) {
            depth += 1;
            if depth > MAX_XML_DEPTH {
                return Err(Error::RecursionLimit {
                    limit: MAX_XML_DEPTH,
                    context: format!("checking the XML nesting depth of {}", path.display()),
                });
            }

            let (end, self_closing) = end_of_tag(bytes, index + 1);
            if self_closing {
                depth -= 1;
            }
            index = end;
        } else {
            // Stray '<'. Again, the parsers job to throw an error about.
            index += 1;
        }
    }

    Ok(())
}

/// Where a start tag ends, and whether it closed itself.
///
/// `from` is the offset just past the  `<`. Quoting is tracked because an attribute value may hold
/// a literal `>`, stopping at the first one would read `<X A="a>b" />` as a tag that opens a level
/// and never closes it.
///
/// A tag left unterminated at the end of input reports the end of input.
fn end_of_tag(bytes: &[u8], from: usize) -> (usize, bool) {
    let mut quote: Option<u8> = None;

    for index in from..bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(delimiter) if byte == delimiter => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            // XML permits no whitespace between the `/` and the `>` of an empty element tag.
            // The byte before is the entire test. `index > from` cannot underflow, as `from` is at
            // least 1, being one past a `<`.
            None if byte == b'>' => return (index + 1, bytes[index - 1] == b'/'),
            None => {}
        }
    }
    (bytes.len(), false)
}

/// The offset just past the next `needle` at or after `from`, or the end of input if `needle` is
/// not found.
fn skip_past(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    match bytes[from..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        Some(offset) => from + offset + needle.len(),
        None => bytes.len(),
    }
}

/// Whether `byte` can begin an XML element name.
///
/// Only approximate for the sake of speed. The real production is a long list of Unicode ranges,
/// and every byte of a multi-byte UTF-8 sequence is `>= 0x80` so accepting all of them is fine.
fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':' || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorKind, Severity};

    /// What Visual Studio stamps on the root element. Hand-written property sheets routinely omit
    /// it, which is why the reader matches on the local name.
    const MSBUILD_NS: &str = "http://schemas.microsoft.com/developer/msbuild/2003";

    /// A rooted project path for the platform under test, matching `sln.rs`'s approach: a
    /// `C:\`-prefixed path is *relative* on Linux CI, and diagnostics are compared against it.
    fn project() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\proj\app\app.vcxproj")
        } else {
            PathBuf::from("/proj/app/app.vcxproj")
        }
    }

    /// Wrap `body` in the `<Project>` element Visual Studio writes.
    ///
    /// The declaration is line 1 and `<Project>` is line 2, so **the body starts at line 3** —
    /// which is what every line-number assertion below counts from.
    fn project_text(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Project DefaultTargets=\"Build\" xmlns=\"{MSBUILD_NS}\">\n\
             {body}\n\
             </Project>\n"
        )
    }

    /// Parse with a quiet reporter, for the majority of tests that assert on the parse itself.
    fn parse(text: &str) -> Result<ProjectFile> {
        parse_project_text(text, &project(), &mut Reporter::new(false))
    }

    /// Parse and hand back the reporter, for the tests that assert on what was warned about.
    fn parse_reporting(text: &str) -> (Result<ProjectFile>, Reporter) {
        let mut reporter = Reporter::new(false);
        let result = parse_project_text(text, &project(), &mut reporter);
        (result, reporter)
    }

    /// The element sequence by kind, which is what the ordering tests actually pin. Comparing
    /// whole `ProjectFile`s would drag line numbers into assertions that do not care about them.
    fn shape(file: &ProjectFile) -> Vec<&'static str> {
        file.elements
            .iter()
            .map(|element| match element {
                Element::Properties(_) => "PropertyGroup",
                Element::ItemDefinitions(_) => "ItemDefinitionGroup",
                Element::Items(_) => "ItemGroup",
                Element::Import(_) => "Import",
                Element::ImportGroup(_) => "ImportGroup",
                Element::Unsupported { .. } => "Unsupported",
            })
            .collect()
    }

    /// `(name, value, condition)` per setting — the parts a test usually cares about, with line
    /// numbers left to `line_numbers_name_the_element_they_came_from`.
    fn triples(settings: &[Setting]) -> Vec<(&str, &str, Option<&str>)> {
        settings
            .iter()
            .map(|setting| {
                (
                    setting.name.as_str(),
                    setting.value.as_str(),
                    setting.condition.as_deref(),
                )
            })
            .collect()
    }

    fn as_property_group(file: &ProjectFile, index: usize) -> &PropertyGroup {
        match &file.elements[index] {
            Element::Properties(group) => group,
            other => panic!("element {index} is not a PropertyGroup: {other:?}"),
        }
    }

    fn as_item_definitions(file: &ProjectFile, index: usize) -> &ItemDefinitionGroup {
        match &file.elements[index] {
            Element::ItemDefinitions(group) => group,
            other => panic!("element {index} is not an ItemDefinitionGroup: {other:?}"),
        }
    }

    fn as_item_group(file: &ProjectFile, index: usize) -> &ItemGroup {
        match &file.elements[index] {
            Element::Items(group) => group,
            other => panic!("element {index} is not an ItemGroup: {other:?}"),
        }
    }

    fn as_import(file: &ProjectFile, index: usize) -> &Import {
        match &file.elements[index] {
            Element::Import(import) => import,
            other => panic!("element {index} is not an Import: {other:?}"),
        }
    }

    fn as_import_group(file: &ProjectFile, index: usize) -> &ImportGroup {
        match &file.elements[index] {
            Element::ImportGroup(group) => group,
            other => panic!("element {index} is not an ImportGroup: {other:?}"),
        }
    }

    #[test]
    fn top_level_elements_keep_document_order() {
        // Interleaved on purpose, with `Import`s either side of the settings: which assignment
        // wins is decided by position, so a reader that grouped by kind would hold every element
        // and still have lost the answer.
        let text = project_text(
            r#"  <PropertyGroup Label="Globals">
    <ProjectGuid>{A1}</ProjectGuid>
  </PropertyGroup>
  <Import Project="$(VCTargetsPath)\Microsoft.Cpp.Default.props" />
  <PropertyGroup Label="Configuration">
    <ConfigurationType>Application</ConfigurationType>
  </PropertyGroup>
  <ItemDefinitionGroup>
    <ClCompile>
      <WarningLevel>Level3</WarningLevel>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemGroup>
    <ClCompile Include="main.cpp" />
  </ItemGroup>
  <Import Project="$(VCTargetsPath)\Microsoft.Cpp.targets" />"#,
        );

        let file = parse(&text).expect("parses");

        assert_eq!(
            shape(&file),
            [
                "PropertyGroup",
                "Import",
                "PropertyGroup",
                "ItemDefinitionGroup",
                "ItemGroup",
                "Import",
            ]
        );
        assert_eq!(file.path, project(), "the file names where it came from");
    }

    #[test]
    fn a_property_assigned_twice_keeps_both_in_order() {
        // Which assignment wins is MVP-08's decision to make. Collecting into a map here would
        // make it instead — invisibly, and with no way for a later pass to recover the loser.
        let text = project_text(
            r#"  <PropertyGroup>
    <ConfigurationType>Application</ConfigurationType>
    <ConfigurationType>StaticLibrary</ConfigurationType>
  </PropertyGroup>"#,
        );

        let file = parse(&text).expect("parses");

        assert_eq!(
            triples(&as_property_group(&file, 0).properties),
            [
                ("ConfigurationType", "Application", None),
                ("ConfigurationType", "StaticLibrary", None),
            ]
        );
    }

    #[test]
    fn a_condition_is_captured_verbatim() {
        // No macro expanded, no whitespace normalized, and the group's condition kept apart from
        // its child's: combining them into `(a) and (b)` would put an expression in the IR that
        // appears in no file.
        let text = project_text(
            r#"  <PropertyGroup Condition="'$(Configuration)|$(Platform)'=='Debug|x64'">
    <UseDebugLibraries Condition=" '$(Foo)' != '' ">true</UseDebugLibraries>
  </PropertyGroup>"#,
        );

        let file = parse(&text).expect("parses");
        let group = as_property_group(&file, 0);

        assert_eq!(
            group.condition.as_deref(),
            Some("'$(Configuration)|$(Platform)'=='Debug|x64'")
        );
        assert_eq!(
            group.properties[0].condition.as_deref(),
            Some(" '$(Foo)' != '' "),
            "even the padding survives — MVP-09 owns what a condition means"
        );
    }

    #[test]
    fn an_escaped_condition_is_decoded_and_not_otherwise_touched() {
        // `&apos;` is how a condition survives XML at all. MVP-09 has to see the string the file
        // means, not the one it spells.
        let text = project_text(
            r#"  <PropertyGroup Condition="&apos;$(Configuration)&apos;==&apos;Debug&apos;" />"#,
        );

        let file = parse(&text).expect("parses");

        assert_eq!(
            as_property_group(&file, 0).condition.as_deref(),
            Some("'$(Configuration)'=='Debug'")
        );
    }

    #[test]
    fn an_unconditional_element_has_no_condition() {
        let text = project_text(r#"  <PropertyGroup><X>1</X></PropertyGroup>"#);

        let file = parse(&text).expect("parses");
        let group = as_property_group(&file, 0);

        assert!(group.condition.is_none());
        assert!(group.properties[0].condition.is_none());
    }

    #[test]
    fn clcompile_items_are_extracted_across_groups_in_order() {
        let text = project_text(
            r#"  <ItemGroup>
    <ClCompile Include="main.cpp" />
    <ClInclude Include="app.h" />
    <ClCompile Include="util.cpp">
      <PreprocessorDefinitions>EXTRA;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
  </ItemGroup>
  <ItemGroup>
    <ClCompile Include="late.cpp" />
  </ItemGroup>"#,
        );

        let file = parse(&text).expect("parses");

        let sources: Vec<&str> = file
            .items_of_type("ClCompile")
            .filter_map(|item| item.include.as_deref())
            .collect();
        assert_eq!(sources, ["main.cpp", "util.cpp", "late.cpp"]);

        let util = file
            .items_of_type("ClCompile")
            .nth(1)
            .expect("three sources");
        assert_eq!(
            triples(&util.metadata),
            [(
                "PreprocessorDefinitions",
                "EXTRA;%(PreprocessorDefinitions)",
                None
            )],
            "`%(...)` is left alone; MVP-12 owns inheritance"
        );

        // The header is recorded, not discarded — it is simply not a source file.
        assert_eq!(as_item_group(&file, 0).items.len(), 3);
        assert_eq!(file.items_of_type("ClInclude").count(), 1);
    }

    #[test]
    fn an_item_type_is_matched_case_insensitively() {
        // MSBuild item names are case-insensitive and `<Clcompile>` builds in Visual Studio.
        // Treating it as a different type would silently drop the source file.
        let text = project_text(r#"  <ItemGroup><Clcompile Include="main.cpp" /></ItemGroup>"#);

        let file = parse(&text).expect("parses");

        assert_eq!(file.items_of_type("ClCompile").count(), 1);
    }

    #[test]
    fn an_items_attributes_are_recorded_unexpanded() {
        let (result, reporter) = parse_reporting(&project_text(
            r#"  <ItemGroup>
    <ClCompile Include="src\*.cpp" Exclude="src\gen.cpp" />
    <ClCompile Update="main.cpp">
      <PrecompiledHeader>Use</PrecompiledHeader>
    </ClCompile>
    <ClCompile Remove="old.cpp" />
  </ItemGroup>"#,
        ));
        let file = result.expect("parses");
        let items: Vec<&Item> = file.items_of_type("ClCompile").collect();

        // Stored as written. Nothing expands the wildcard yet — a known gap, see ADR 07.
        assert_eq!(items[0].include.as_deref(), Some(r"src\*.cpp"));
        assert_eq!(items[0].exclude.as_deref(), Some(r"src\gen.cpp"));
        // `Update` attaches metadata to an item declared elsewhere, which is one way per-file
        // compiler settings reach MVP-12.
        assert_eq!(items[1].update.as_deref(), Some("main.cpp"));
        assert_eq!(
            triples(&items[1].metadata),
            [("PrecompiledHeader", "Use", None)]
        );
        assert_eq!(items[2].remove.as_deref(), Some("old.cpp"));

        // Each of the three names something, so none of them is the "configures nothing" case.
        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    #[test]
    fn project_configuration_items_come_out_of_the_same_accessor() {
        // The `Label="ProjectConfigurations"` group declares which pairs the project supports.
        // Nothing special-cases it: it is an `ItemGroup` of items with metadata like any other.
        let text = project_text(
            r#"  <ItemGroup Label="ProjectConfigurations">
    <ProjectConfiguration Include="Debug|Win32">
      <Configuration>Debug</Configuration>
      <Platform>Win32</Platform>
    </ProjectConfiguration>
  </ItemGroup>"#,
        );

        let file = parse(&text).expect("parses");
        let declared: Vec<&Item> = file.items_of_type("ProjectConfiguration").collect();

        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].include.as_deref(), Some("Debug|Win32"));
        assert_eq!(
            triples(&declared[0].metadata),
            [
                ("Configuration", "Debug", None),
                ("Platform", "Win32", None)
            ]
        );
    }

    #[test]
    fn item_definition_metadata_is_read_per_item_type() {
        // Where nearly every flag lives: `ClCompile` sets the project-wide defaults MVP-12 turns
        // into include paths and defines, and `Link` sits beside it, recorded and unread.
        let text = project_text(
            r#"  <ItemDefinitionGroup Condition="'$(Configuration)'=='Debug'">
    <ClCompile>
      <AdditionalIncludeDirectories>$(ProjectDir)inc;%(AdditionalIncludeDirectories)</AdditionalIncludeDirectories>
      <PreprocessorDefinitions>WIN32;_DEBUG;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
    <Link>
      <SubSystem>Console</SubSystem>
    </Link>
  </ItemDefinitionGroup>"#,
        );

        let file = parse(&text).expect("parses");
        let group = as_item_definitions(&file, 0);

        assert_eq!(
            group.condition.as_deref(),
            Some("'$(Configuration)'=='Debug'")
        );
        assert_eq!(
            group
                .definitions
                .iter()
                .map(|definition| definition.item_type.as_str())
                .collect::<Vec<_>>(),
            ["ClCompile", "Link"]
        );
        assert_eq!(
            triples(&group.definitions[0].metadata),
            [
                (
                    "AdditionalIncludeDirectories",
                    "$(ProjectDir)inc;%(AdditionalIncludeDirectories)",
                    None
                ),
                (
                    "PreprocessorDefinitions",
                    "WIN32;_DEBUG;%(PreprocessorDefinitions)",
                    None
                ),
            ]
        );
    }

    #[test]
    fn an_import_group_keeps_its_own_condition_and_its_children() {
        // The `PropertySheets` group every Visual Studio project carries. The group's condition
        // gates every child and each child carries its own; both are needed, separately.
        let text = project_text(
            r#"  <ImportGroup Label="PropertySheets" Condition="'$(Configuration)'=='Debug'">
    <Import Project="$(UserRootDir)\Microsoft.Cpp.$(Platform).user.props" Condition="exists('$(UserRootDir)\Microsoft.Cpp.$(Platform).user.props')" />
    <Import Project="..\shared\common.props" />
  </ImportGroup>"#,
        );

        let file = parse(&text).expect("parses");
        let group = as_import_group(&file, 0);

        assert_eq!(
            group.condition.as_deref(),
            Some("'$(Configuration)'=='Debug'")
        );
        assert_eq!(
            group
                .imports
                .iter()
                .map(|import| import.project.as_str())
                .collect::<Vec<_>>(),
            [
                r"$(UserRootDir)\Microsoft.Cpp.$(Platform).user.props",
                r"..\shared\common.props",
            ]
        );
        assert!(group.imports[0].condition.is_some());
        assert!(
            group.imports[1].condition.is_none(),
            "an unconditional import must not inherit the group's condition"
        );
    }

    #[test]
    fn an_empty_group_is_kept_rather_than_dropped() {
        // Visual Studio writes `<ImportGroup Label="ExtensionSettings">` and
        // `<PropertyGroup Label="UserMacros" />` with nothing in them. An element with no children
        // is not an absent element.
        let text = project_text(
            r#"  <ImportGroup Label="ExtensionSettings">
  </ImportGroup>
  <PropertyGroup Label="UserMacros" />"#,
        );

        let file = parse(&text).expect("parses");

        assert_eq!(shape(&file), ["ImportGroup", "PropertyGroup"]);
        assert!(as_import_group(&file, 0).imports.is_empty());
        assert!(as_property_group(&file, 1).properties.is_empty());
    }

    #[test]
    fn a_value_split_across_lines_is_trimmed() {
        // Hand-formatted sheets wrap long lists. MSBuild preserves the whitespace; keeping it here
        // would leave a trailing newline inside a directory-valued property and corrupt every path
        // concatenated from it.
        let text = project_text(
            "  <PropertyGroup>\n    \
             <OutDir>\n      $(SolutionDir)bin\\\n    </OutDir>\n    \
             <Empty></Empty>\n    \
             <SelfClosing />\n  \
             </PropertyGroup>",
        );

        let file = parse(&text).expect("parses");

        assert_eq!(
            triples(&as_property_group(&file, 0).properties),
            [
                ("OutDir", r"$(SolutionDir)bin\", None),
                ("Empty", "", None),
                ("SelfClosing", "", None),
            ]
        );
    }

    #[test]
    fn the_msbuild_namespace_is_optional() {
        // Visual Studio always writes `xmlns`; hand-written property sheets routinely omit it, and
        // MSBuild reads both. Line numbers differ between the two spellings, so the comparison is
        // on shape and content rather than on whole `ProjectFile`s.
        let body = r#"  <PropertyGroup>
    <ConfigurationType>Application</ConfigurationType>
  </PropertyGroup>
  <ItemGroup>
    <ClCompile Include="main.cpp" />
  </ItemGroup>"#;

        let with_ns = parse(&project_text(body)).expect("parses");
        let without_ns = parse(&format!("<Project>\n{body}\n</Project>\n")).expect("parses");

        assert_eq!(shape(&with_ns), shape(&without_ns));
        assert_eq!(
            triples(&as_property_group(&with_ns, 0).properties),
            triples(&as_property_group(&without_ns, 0).properties)
        );
        assert_eq!(
            without_ns.items_of_type("ClCompile").count(),
            1,
            "item types are local names too"
        );
    }

    #[test]
    fn an_unmodelled_element_is_recorded_and_warned_about() {
        // `<Choose>` decides *which* properties apply. Dropping it in silence is exactly how a
        // plausible-but-wrong compilation database gets built, so it stays in the IR and says so.
        let text = project_text(
            r#"  <Choose>
    <When Condition="'$(Platform)'=='x64'">
      <PropertyGroup>
        <Bits>64</Bits>
      </PropertyGroup>
    </When>
  </Choose>"#,
        );

        let (result, reporter) = parse_reporting(&text);
        let file = result.expect("parses anyway");

        // The nested `PropertyGroup` is deliberately *not* lifted out: it is gated by a `When`
        // this reader cannot evaluate, so hoisting it would apply it unconditionally.
        assert_eq!(
            file.elements,
            [Element::Unsupported {
                name: "Choose".to_string(),
                line: 3,
            }]
        );

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.line, Some(3));
        assert_eq!(warning.file.as_deref(), Some(project().as_path()));
        assert!(
            warning.message.contains("Choose"),
            "names the element: {}",
            warning.message
        );
    }

    #[test]
    fn build_only_elements_are_skipped_without_a_warning() {
        // A `Target` runs during a build and `ProjectExtensions` is IDE bookkeeping; neither can
        // change how a source file compiles. Skipped in silence the way `sln.rs` skips a solution
        // folder — warning about correct, common content would devalue every other warning.
        let text = project_text(
            r#"  <Target Name="AfterBuild">
    <Message Text="done" />
  </Target>
  <UsingTask TaskName="X" AssemblyFile="x.dll" />
  <ProjectExtensions>
    <VisualStudio />
  </ProjectExtensions>
  <PropertyGroup />"#,
        );

        let (result, reporter) = parse_reporting(&text);
        let file = result.expect("parses");

        assert_eq!(shape(&file), ["PropertyGroup"]);
        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    /// A *note*, per MVP-13: an item with no `Include`, `Remove` or `Update` configures nothing,
    /// so nothing was lost by not modelling it. Worth saying, not worth reading as "your flags
    /// may be wrong" — which is the line ADR 13 draws.
    #[test]
    fn an_item_naming_nothing_notes_but_still_parses() {
        let (result, reporter) = parse_reporting(&project_text(
            r#"  <ItemGroup>
    <ClCompile />
  </ItemGroup>"#,
        ));
        let file = result.expect("parses anyway");

        assert_eq!(
            file.items_of_type("ClCompile").count(),
            1,
            "the note is not a refusal"
        );

        let [finding] = reporter.warnings() else {
            panic!("expected exactly one finding: {:?}", reporter.warnings());
        };
        assert_eq!(finding.severity, Severity::Note);
        assert_eq!(finding.line, Some(4));
    }

    #[test]
    fn a_non_import_inside_an_import_group_warns_and_is_skipped() {
        let (result, reporter) = parse_reporting(&project_text(
            r#"  <ImportGroup>
    <PropertyGroup />
    <Import Project="a.props" />
  </ImportGroup>"#,
        ));
        let file = result.expect("parses anyway");

        assert_eq!(as_import_group(&file, 0).imports.len(), 1);
        assert_eq!(reporter.warnings().len(), 1);
        // A warning rather than a note: an `ImportGroup` is the mechanism whole property sheets
        // arrive through, and this is content inside one that was skipped.
        assert_eq!(reporter.warnings()[0].severity, Severity::Warning);
        assert_eq!(reporter.warnings()[0].line, Some(4));
    }

    /// The one place this reader used to drop content without saying so, closed by MVP-13.
    ///
    /// `text_of` keeps a value's text and discards any element children. That is the right
    /// recovery — the construct is not valid MSBuild and there is nothing else to do with them —
    /// but silently losing part of a value that becomes a compiler flag is exactly what this
    /// issue exists to stop.
    #[test]
    fn a_value_with_element_children_warns_and_keeps_its_text() {
        let (result, reporter) = parse_reporting(&project_text(
            r#"  <PropertyGroup>
    <PreprocessorDefinitions>WIN32<Nested /></PreprocessorDefinitions>
  </PropertyGroup>"#,
        ));
        let file = result.expect("parses anyway");

        assert_eq!(
            as_property_group(&file, 0).properties[0].value,
            "WIN32",
            "the text survives; only the elements are dropped"
        );

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.line, Some(4));
        assert!(
            warning.message.contains("PreprocessorDefinitions"),
            "names the value that lost content: {}",
            warning.message
        );
    }

    /// The negative case, so the check above cannot be satisfied by warning on every nesting.
    /// Both shapes here are ordinary: a `ProjectConfiguration`'s `<Configuration>`/`<Platform>`
    /// children and an `ItemDefinitionGroup`'s `<ClCompile>` metadata are both reached *through*
    /// `settings` rather than seen by it, so only a genuine third level is a finding.
    #[test]
    fn the_ordinary_two_level_shapes_do_not_warn() {
        let (result, reporter) = parse_reporting(&project_text(
            r#"  <ItemGroup Label="ProjectConfigurations">
    <ProjectConfiguration Include="Debug|x64">
      <Configuration>Debug</Configuration>
      <Platform>x64</Platform>
    </ProjectConfiguration>
  </ItemGroup>
  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>WIN32</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>"#,
        ));
        result.expect("parses");

        assert!(reporter.warnings().is_empty(), "{:?}", reporter.warnings());
    }

    #[test]
    fn line_numbers_name_the_element_they_came_from() {
        // Counted against the literal below, where the body starts at line 3. Diagnostics and
        // errors are only actionable if these are right, and nothing else asserts them.
        let text = project_text(
            r#"  <PropertyGroup>
    <ConfigurationType>Application</ConfigurationType>
  </PropertyGroup>
  <Import Project="a.props" />
  <ItemGroup>
    <ClCompile Include="main.cpp" />
  </ItemGroup>"#,
        );

        let file = parse(&text).expect("parses");

        assert_eq!(as_property_group(&file, 0).line, 3);
        assert_eq!(as_property_group(&file, 0).properties[0].line, 4);
        assert_eq!(as_import(&file, 1).line, 6);
        assert_eq!(as_item_group(&file, 2).line, 7);
        assert_eq!(as_item_group(&file, 2).items[0].line, 8);
    }

    #[test]
    fn a_utf8_bom_does_not_prevent_parsing() {
        // Visual Studio writes one. `roxmltree` skips it itself, which is why there is no
        // counterpart here to the manual strip in `sln.rs`.
        let text = format!("\u{feff}{}", project_text(r#"  <PropertyGroup />"#));

        let file = parse(&text).expect("parses");

        assert_eq!(shape(&file), ["PropertyGroup"]);
        assert_eq!(
            as_property_group(&file, 0).line,
            3,
            "the BOM must not shift line numbers"
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        let cases = [
            ("<Project><PropertyGroup></Project>", "mismatched tag"),
            ("<Project>", "unclosed root"),
            ("", "empty input"),
            ("not xml at all", "not markup"),
            (
                r#"<Project><PropertyGroup Condition="unterminated /></Project>"#,
                "unterminated attribute",
            ),
        ];

        for (text, what) in cases {
            let error = parse(text).expect_err("must not parse");
            assert_eq!(error.kind(), ErrorKind::ProjectParse, "for {what}");
        }
    }

    #[test]
    fn a_root_element_that_is_not_project_is_rejected() {
        // A `.vcxproj.filters`, or any XML file handed to the wrong argument. Well-formed, so
        // `roxmltree` is content; there is simply nothing here to evaluate.
        let error = parse("<Filters><ItemGroup /></Filters>").expect_err("must not parse");

        assert_eq!(
            error.kind(),
            ErrorKind::ProjectSchema,
            "distinct from `project-parse`: the XML is fine, the content is not"
        );
        assert!(
            error.to_string().contains("Filters"),
            "names the element: {error}"
        );
        assert!(error.to_string().contains(":1:"), "names the line: {error}");
    }

    #[test]
    fn an_import_without_a_project_attribute_is_rejected() {
        // `<Import Sdk="..."/>` included: an import is how the entire flag set arrives, so one
        // that resolves to nothing leaves a database quietly missing settings.
        let error = parse(&project_text(r#"  <Import Sdk="Microsoft.NET.Sdk" />"#))
            .expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::ProjectSchema);
        assert!(error.to_string().contains(":3:"), "names the line: {error}");
    }

    #[test]
    fn a_dtd_is_rejected() {
        // The entity-expansion surface. Project files are untrusted input (CLAUDE.md §5) and have
        // no legitimate use for a DTD, so `roxmltree`'s defaults — which refuse one — are never
        // overridden.
        let text = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<!DOCTYPE Project [<!ENTITY a \"aaaaaaaaaa\">]>\n",
            "<Project><PropertyGroup><X>&a;</X></PropertyGroup></Project>",
        );

        let error = parse(text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::ProjectParse);
    }

    /// CLAUDE.md §5: hostile input must fail gracefully rather than hang or panic.
    ///
    /// `roxmltree::Document::parse` recurses once per nesting level, and a debug build on a 2 MiB
    /// thread stack dies somewhere between 100 and 200 levels — around 1.5 KB of XML. The whole
    /// process aborted, so there was no error, no exit code, and nothing for the plugin to report.
    /// The lowering below the parse is iterative and was never implicated.
    ///
    /// Written and `#[ignore]`d by MVP-07 because running it took the test binary with it, and
    /// un-ignored by MVP-35, which bounds depth before the text reaches `roxmltree`. This is the
    /// test the whole guard exists for: it must reach an `Err` at all, which is the part that used
    /// to be impossible.
    #[test]
    fn a_deeply_nested_document_is_rejected_rather_than_crashing() {
        const DEPTH: usize = 5_000;
        let text = format!(
            "<Project><PropertyGroup>{}{}</PropertyGroup></Project>",
            "<X>".repeat(DEPTH),
            "</X>".repeat(DEPTH),
        );

        let error = parse(&text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn a_document_at_the_nesting_limit_still_parses() {
        // The cap is a ceiling, not a limit below it — the same thing `imports.rs` and `eval.rs`
        // assert about theirs. `<Project>` is level 1, so the body may nest one less than the cap.
        //
        // Built through `project_text`, so this doubles as the assertion that the `<?xml ... ?>`
        // declaration is not counted as a level: if it were, this document would be one over.
        let inner = MAX_XML_DEPTH - 1;
        let text = project_text(&format!(
            "  {}{}",
            "<X>".repeat(inner),
            "</X>".repeat(inner)
        ));

        parse(&text).expect("a document exactly at the cap is not over it");
    }

    #[test]
    fn a_document_one_level_past_the_limit_is_refused() {
        // The other side of the boundary, so an off-by-one in either direction has to fail one of
        // this pair rather than hiding between them.
        let inner = MAX_XML_DEPTH;
        let text = project_text(&format!(
            "  {}{}",
            "<X>".repeat(inner),
            "</X>".repeat(inner)
        ));

        let error = parse(&text).expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
        assert!(
            error.to_string().contains("app.vcxproj"),
            "names the file that was refused: {error}"
        );
    }

    #[test]
    fn markup_inside_a_comment_does_not_count_toward_depth() {
        // The guard is a byte scan, so it sees a `<` without knowing what encloses it. A comment
        // holding unbalanced tags is legal XML, and a commented-out `ItemGroup` is an ordinary
        // thing to find in a real project file.
        let text = project_text(&format!(
            "  <!-- {} -->\n  <PropertyGroup />",
            "<X>".repeat(MAX_XML_DEPTH * 2)
        ));

        let file = parse(&text).expect("a comment nests nothing");

        assert_eq!(shape(&file), ["PropertyGroup"]);
    }

    #[test]
    fn markup_inside_cdata_does_not_count_toward_depth() {
        // Same hazard as the comment above, different terminator — and unlike a comment, a CDATA
        // section is content the reader goes on to keep.
        let text = project_text(&format!(
            "  <PropertyGroup>\n    <Banner><![CDATA[{}]]></Banner>\n  </PropertyGroup>",
            "<X>".repeat(MAX_XML_DEPTH * 2)
        ));

        let file = parse(&text).expect("CDATA is text, not nesting");

        assert_eq!(
            as_property_group(&file, 0).properties[0].name,
            "Banner",
            "the element around the CDATA is still read normally"
        );
    }

    #[test]
    fn self_closing_siblings_do_not_accumulate_depth() {
        // The false-positive case, and the one that matters most: every real project lists its
        // sources as self-closing `<ClCompile Include="..." />` items. A scan that counted `<`
        // without noticing `/>` would leak a level each and refuse ordinary solutions long before
        // it refused a hostile file.
        let count = MAX_XML_DEPTH * 4;
        let items: String = (0..count)
            .map(|n| format!("    <ClCompile Include=\"src{n}.cpp\" />\n"))
            .collect();
        let text = project_text(&format!("  <ItemGroup>\n{items}  </ItemGroup>"));

        let file = parse(&text).expect("siblings nest no deeper than one of them");

        assert_eq!(file.items_of_type("ClCompile").count(), count);
    }

    #[test]
    fn a_greater_than_inside_an_attribute_value_does_not_end_the_tag() {
        // XML forbids a literal `<` in an attribute value but permits a literal `>`, which is why
        // finding where a tag *ends* needs the quoting tracked even though finding where one
        // begins does not. A scan that stopped at the first `>` would read each item below as an
        // unclosed tag and leak a level per source file. Visual Studio writes `&gt;` — this is
        // the spelling a hand-written sheet is allowed to use.
        let count = MAX_XML_DEPTH * 4;
        let items: String = (0..count)
            .map(|n| {
                format!("    <ClCompile Include=\"src{n}.cpp\" Condition=\"'$(N)' != 'a>b'\" />\n")
            })
            .collect();
        let text = project_text(&format!("  <ItemGroup>\n{items}  </ItemGroup>"));

        let file = parse(&text).expect("a `>` in an attribute value is not the end of the tag");

        assert_eq!(file.items_of_type("ClCompile").count(), count);
    }

    #[test]
    fn a_missing_project_file_is_an_io_error() {
        // Not `MissingImport`: only MVP-10 knows which file referenced this one, so naming the
        // referrer is its job rather than the reader's.
        let error = parse_project(
            Path::new("no-such-project.vcxproj"),
            &mut Reporter::new(false),
        )
        .expect_err("must not parse");

        assert_eq!(error.kind(), ErrorKind::Io);
    }
}
