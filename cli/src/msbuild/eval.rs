//! Macro expansion and `Condition` evaluation.
//!
//! Handles MSBuild property references such as `$(Configuration)`, `$(Platform)`,
//! `$(SolutionDir)`, and conditions like
//! `Condition="'$(Configuration)|$(Platform)'=='Debug|x64'"`.
//!
//! Expansion is **eager**. A value is expanded when it is assigned, against the contents of the bag
//! as they stand in the moment of assignment. What is stored contains no `$(` at all. That is
//! according to MSBuilds own rulesets. See [ADR-08](../../../docs/adr/08-eager-macro-expansion.md).
//!
//! Unsupported constructs must be surfaced as warnings rather than silently producing
//! incorrect flags — see the engineering requirements in docs/engineering_spec.md.

use std::collections::HashMap;

use crate::{Error, Result};

/// How large one expanded value may grow before the expansion is refused.
///
/// Eager expansion makes cycles impossible, but not runaway growth.
const MAX_VALUE_BYTES: usize = 1 << 20;

/// The property bag built up during evaluation. Later assignments shadow earlier ones.
#[derive(Debug, Default, Clone)]
pub struct Properties {
    values: HashMap<String, String>,
}

impl Properties {
    pub fn new() -> Self {
        Properties::default()
    }

    /// The value of `name`, or `None` if nothing was ever assigned to the key.
    ///
    /// `Some("")` and `None` are deliberately different. A property assigned an empty value is
    /// *defined*, and treating it as undefined would induce warnings about 
    /// `Condition="'$(X)'" != ''"`, which is how all real projects check for a property it expects
    /// to be absent.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Store `value` as given, shadowing any earlier assignment.
    ///
    /// Raw, because MVP-11 will seed reserved properties whose values are literal paths with
    /// nothing to expand. Anything read out a project file goes through [`Properties::assign`].
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.values.insert(name.to_ascii_lowercase(), value.into());
    }

    /// Expand `value` against the contents of the bag at this moment, then store the result.
    ///
    /// The returned [`Expansion`] carries what could not be resolved, for the caller to report
    /// warnings with.
    pub fn assign(&mut self, name: &str, value: &str) -> Result<Expansion> {
        let expansion = expand(value, self)?;
        self.set(name, expansion.value.clone());
        Ok(expansion)
    }
}

/// One expanded value, along with anything that could not be resolved while expanding it.
///
/// The unexpanded values are there for MVP-13 to use for building warning messages. Both unexpanded
/// value lists are deduplicated and stored in first-reference order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[must_use]
pub struct Expansion {
    /// The text with every supported reference replaced
    pub value: String,
    /// `$(Name)` references the bag had no value for. Each expanded to nothing, as MSBuild does.
    pub undefined: Vec<String>,
    /// References outside the supported subset -- property functions, registry reads, nested
    /// references, an unterminated `$(`. Stored exactly as seen in `value`, sigil (`$()`) included.
    pub unsupported: Vec<String>,
}

impl Expansion {
    /// Record an undefined reference, deduping case-insensitively.
    fn record_undefined(&mut self, name: &str) {
        if !self.undefined.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            self.undefined.push(name.to_string());
        }
    }

    fn record_unsupported(&mut self, name: &str) {
        if !self.unsupported.iter().any(|seen| seen == name ) {
            self.unsupported.push(name.to_string());
        }
    }
}

/// Expand `$(...)` references in `input` against the current property bag.
///
/// One pass, not recursive deliberately. Values in the bag were expanded when they were assigned,
/// so a substituted value cannot contain references. Rescanning would possibly introduce cycles.
///
/// `%(Metadata)` and `@(Item)` are left alone. `$` is the only sigil handled here and `%(...)`
/// inheritance will be completed in MVP-12.
pub fn expand(input: &str, props: &Properties) -> Result<Expansion> {
    let mut out = Expansion::default();
    let mut rest = input;

    while let Some(open) = rest.find("$(") {
        out.value.push_str(&rest[..open]);
        let body_start = open + 2;

        let Some(offset) = closing_paren(&rest[body_start..]) else {
            // MSBuild fails the build on an unterminated reference. Record and copied through
            // instead. One stray `$(` in a property nothing reads shouldn't kill the entire
            // generation when we have warnings to the user to indicate their fuck-up.
            out.record_unsupported(&rest[open..]);
            out.value.push_str(&rest[open..]);
            rest = "";
            break;
        };

        let close = body_start + offset;

        // Contents of the `$(..)`
        let body = &rest[body_start..close];

        // Contents of the `$()`, including sigil characters
        let reference = &rest[open..close+1];

        if is_property_name(body) {
            match props.get(body) {
                Some(value) => out.value.push_str(value),
                // Expands to nothing, as MSBuild does, but recorded because an unset property in
                // the include path is a likely cause of a database that looks good but isn't
                // functioning properly.
                None => out.record_undefined(body),
            }
        } else {
            // Property function, registry read, nested reference or `$()`. Left in verbatim.
            out.record_unsupported(reference);
            out.value.push_str(reference);
        }

        check_size(&out.value, input)?;
        rest = &rest[close + 1..];
    }

    out.value.push_str(rest);
    check_size(&out.value, input)?;
    Ok(out)
}

/// Evaluate an MSBuild `Condition` expression to a boolean.
pub fn condition_holds(_condition: &str, _props: &Properties) -> Result<bool> {
    todo!("evaluate MSBuild condition")
}

/// The offset of the `)` closing a reference that starts immediately before `data`.
///
/// Tracks depth of the closing `)`. `(` and `)` are ASCII, so byte offsets are char boundaries.
fn closing_paren(data: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in data.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' if depth == 0 => return Some(offset),
            b')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Whether `body` names a property, as opposed to invoking something.
///
/// Property names are XML element names, so `-` is legal. `.` is excluded deliberately: 
/// `$(Foo.Substring(0))` is a property function, and admitting the dot would turn an unsupported
/// value into a lookup of a property that does not exist.
fn is_property_name(body: &str) -> bool {
    let mut chars = body.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_') &&
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Error if a value has grown past [`MAX_VALUE_BYTES`].
///
/// Can't really error on recursion limit, so we error out if a value has grown beyond a certain
/// size as a way to contain malicious project files.
fn check_size(value: &str, input: &str) -> Result<()> {
    if value.len() <= MAX_VALUE_BYTES {
        return Ok(())
    }

    Err(Error::RecursionLimit { limit: MAX_VALUE_BYTES, context: format!(
            "expanding `{}` into a value of more than {MAX_VALUE_BYTES} bytes",
            truncate(input, 60)
    ) })
}

// Shorten a value for a message.
fn truncate(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}...", &input[..cut]),
        None => input.to_string(),
    }
}


