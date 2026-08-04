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
    Unsupported { name: String, line: usize },
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
    /// Raw and unexpanded: `$(VCTargetsPath)\Microsoft.cpp.props` only resolves once MVP-11 has
    /// seeded the reserved properties.
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
                _ => None
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
    let text = std::fs::read_to_string(path).map_err(|err| Error::io(path,err))?;
    parse_project_text(&text, path, reporter)
}

/// Parse project XML text. 
///
/// `path` names the file in errors and diagnostics. Nothing is read from the disk here
fn parse_project_text(text: &str, path: &Path, reporter: &mut Reporter) -> Result<ProjectFile> {
    let doc = Document::parse(text).map_err(|err| Error::ProjectParse { 
        path: path.to_path_buf(), 
        source: err 
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
                properties: settings(&doc, child),
                line,
            }),
            "ItemDefinitionGroup" => Element::ItemDefinitions(ItemDefinitionGroup { 
                condition,
                definitions: item_definitions(&doc, child), 
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
                    format!("`<{name}>` is not supported; whatever it configures will be ignored")
                );
                Element::Unsupported { name: name.to_string(), line }
            }
        };

        elements.push(element);
    }

    progress!(reporter,
        "{}: {} top-level element(s)",
        path.display(),
        elements.len()
    );

    Ok( ProjectFile {
        path: path.to_path_buf(),
        elements
    })
}


/// The `<Name>value</Name>` children of a group, in document order.
///
/// Duplicates are retained. A `PropertyGroup` may assign the same name twice and the last one wins.
fn settings(doc: &Document, parent: Node) -> Vec<Setting> {
    parent
        .children()
        .filter(Node::is_element)
        .map(|child| Setting {
            name: child.tag_name().name().to_string(),
            value: text_of(child),
            condition: attribute(child, "Condition"),
            line: line_of(doc, child)
        })
    .collect()
} 

fn item_definitions(doc: &Document, parent: Node) -> Vec<ItemDefinition> {
    parent
        .children()
        .filter(Node::is_element)
        .map(|child| ItemDefinition {
            item_type: child.tag_name().name().to_string(),
            condition: attribute(child, "Condition"),
            metadata: settings(doc, child),
            line: line_of(doc, child)
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
                metadata: settings(doc, child),
                line: line_of(doc, child),
            };

            if item.include.is_none() && item.exclude.is_none() && item.update.is_none() {
                reporter.warn_at(path, Some(item.line), 
                    format!("`<{}>` names no Include, Remove or Update and so configures nothing",
                        item.item_type)
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
fn imports(doc: &Document, parent: Node, path: &Path, reporter: &mut Reporter) -> Result<Vec<Import>> {
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
                )
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
