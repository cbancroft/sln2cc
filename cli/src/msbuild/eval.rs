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
        if !self
            .undefined
            .iter()
            .any(|seen| seen.eq_ignore_ascii_case(name))
        {
            self.undefined.push(name.to_string());
        }
    }

    fn record_unsupported(&mut self, name: &str) {
        if !self.unsupported.iter().any(|seen| seen == name) {
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
        let reference = &rest[open..close + 1];

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
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Error if a value has grown past [`MAX_VALUE_BYTES`].
///
/// Can't really error on recursion limit, so we error out if a value has grown beyond a certain
/// size as a way to contain malicious project files.
fn check_size(value: &str, input: &str) -> Result<()> {
    if value.len() <= MAX_VALUE_BYTES {
        return Ok(());
    }

    Err(Error::RecursionLimit {
        limit: MAX_VALUE_BYTES,
        context: format!(
            "expanding `{}` into a value of more than {MAX_VALUE_BYTES} bytes",
            truncate(input, 60)
        ),
    })
}

// Shorten a value for a message.
fn truncate(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}...", &input[..cut]),
        None => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    /// A bag seeded verbatim — the shape MVP-11 will hand over.
    fn bag(pairs: &[(&str, &str)]) -> Properties {
        let mut props = Properties::new();
        for (name, value) in pairs {
            props.set(name, *value);
        }
        props
    }

    /// Assign, discarding the recording — for the cases asserting on the bag rather than on what
    /// could not be resolved. `Expansion` is `#[must_use]`, which is what makes this worth having.
    fn assign(props: &mut Properties, name: &str, value: &str) {
        let _ = props.assign(name, value).expect("expands");
    }

    /// The expanded text alone, asserting nothing was recorded — which is what most cases want.
    fn expanded(input: &str, props: &Properties) -> String {
        let expansion = expand(input, props).expect("expands");
        assert!(
            expansion.undefined.is_empty(),
            "unexpected undefined: {:?}",
            expansion.undefined
        );
        assert!(
            expansion.unsupported.is_empty(),
            "unexpected unsupported: {:?}",
            expansion.unsupported
        );
        expansion.value
    }

    #[test]
    fn a_reference_is_replaced_by_its_value() {
        let props = bag(&[("ProjectDir", r"C:\proj\app\")]);

        assert_eq!(expanded("$(ProjectDir)inc", &props), r"C:\proj\app\inc");
    }

    #[test]
    fn several_references_expand_in_one_value() {
        // The shape every `Condition` in every Visual Studio project is built from, and the one
        // MVP-09 will hand straight back here.
        let props = bag(&[("Configuration", "Debug"), ("Platform", "x64")]);

        assert_eq!(
            expanded("'$(Configuration)|$(Platform)'", &props),
            "'Debug|x64'"
        );
        assert_eq!(
            expanded("$(Configuration)$(Platform)", &props),
            "Debugx64",
            "adjacent references need no separator between them"
        );
    }

    #[test]
    fn text_with_no_reference_passes_through() {
        let props = Properties::new();

        assert_eq!(
            expanded("WIN32;_DEBUG;_CONSOLE", &props),
            "WIN32;_DEBUG;_CONSOLE"
        );
        assert_eq!(expanded("", &props), "");
    }

    #[test]
    fn property_names_are_matched_case_insensitively() {
        // MSBuild property names are case-insensitive. Landing `$(configuration)` in a different
        // slot than `$(Configuration)` would surface as an empty include path — a database that
        // looks fine and is wrong, which is the failure mode CLAUDE.md §3 cares most about.
        let props = bag(&[("Configuration", "Debug")]);

        assert_eq!(expanded("$(CONFIGURATION)", &props), "Debug");
        assert_eq!(expanded("$(configuration)", &props), "Debug");

        // Assignment collides on the same key, so a redefinition in different casing shadows.
        let mut props = Properties::new();
        props.set("OutDir", "first");
        props.set("outdir", "second");
        assert_eq!(props.get("OUTDIR"), Some("second"));
    }

    #[test]
    fn a_non_ascii_value_expands_without_slicing_mid_character() {
        // The scan works in byte offsets. A multi-byte path in a substituted value must not put
        // one of those offsets inside a character.
        let props = bag(&[("ProjectDir", "C:\\проект\\")]);

        assert_eq!(expanded("$(ProjectDir)inc", &props), "C:\\проект\\inc");
    }

    // The eager-expansion trio. These are the tests that would fail under lazy expansion, and
    // they are why ADR 08 exists.

    #[test]
    fn a_value_is_expanded_when_assigned_not_when_read() {
        // MSBuild expands at the point of assignment. Redefining `$(A)` afterwards must not reach
        // back into a value already built from it — lazy expansion would answer "2x" here.
        let mut props = Properties::new();
        props.set("A", "1");
        assign(&mut props, "B", "$(A)x");
        props.set("A", "2");

        assert_eq!(props.get("B"), Some("1x"));
        assert_eq!(props.get("A"), Some("2"));
    }

    #[test]
    fn a_later_assignment_shadows_an_earlier_one() {
        // The decision MVP-07 deliberately left open by keeping both assignments in the IR.
        let mut props = Properties::new();
        assign(&mut props, "ConfigurationType", "Application");
        assign(&mut props, "ConfigurationType", "StaticLibrary");

        assert_eq!(props.get("ConfigurationType"), Some("StaticLibrary"));
    }

    #[test]
    fn a_property_can_append_to_itself() {
        // The standard MSBuild append idiom, and the thing a depth-limited lazy expander would
        // reject as a cycle. It terminates here because `$(PreprocessorDefinitions)` reads the
        // *previous* value, which contains no reference at all.
        let mut props = Properties::new();
        props.set("PreprocessorDefinitions", "WIN32");
        assign(
            &mut props,
            "PreprocessorDefinitions",
            "$(PreprocessorDefinitions);_DEBUG",
        );
        assign(
            &mut props,
            "PreprocessorDefinitions",
            "$(PreprocessorDefinitions);UNICODE",
        );

        assert_eq!(
            props.get("PreprocessorDefinitions"),
            Some("WIN32;_DEBUG;UNICODE")
        );
    }

    #[test]
    fn a_stored_value_is_never_expanded_twice() {
        // Only raw `set` can put a `$(` into the bag — MVP-11 seeding a literal. Reading it back
        // must not expand it: re-scanning substituted text is the one door back to the cycles
        // eager expansion closes.
        let mut props = Properties::new();
        props.set("Literal", "$(Other)");
        props.set("Other", "boom");

        assert_eq!(expanded("$(Literal)", &props), "$(Other)");
    }

    #[test]
    fn an_undefined_reference_expands_to_nothing_and_is_recorded() {
        // Empty is what MSBuild produces, so producing anything else would be inventing. The
        // recording is what stops it being silent — MVP-13 turns it into the warning.
        let expansion = expand("$(NoSuchProperty)inc", &Properties::new()).expect("expands");

        assert_eq!(expansion.value, "inc");
        assert_eq!(expansion.undefined, ["NoSuchProperty"]);
        assert!(expansion.unsupported.is_empty());
    }

    #[test]
    fn a_property_assigned_an_empty_value_is_not_undefined() {
        // `<PropertyGroup Label="UserMacros" />` and `<Empty></Empty>` are ordinary. Conflating
        // "assigned nothing" with "never assigned" would have MVP-13 warn on every idiomatic
        // `Condition="'$(X)' != ''"`, which is how projects test for an absent property.
        let props = bag(&[("Empty", "")]);
        let expansion = expand("[$(Empty)]", &props).expect("expands");

        assert_eq!(expansion.value, "[]");
        assert!(
            expansion.undefined.is_empty(),
            "a property assigned an empty value is defined"
        );
        assert_eq!(props.get("Empty"), Some(""));
        assert_eq!(props.get("Missing"), None);
    }

    #[test]
    fn undefined_names_are_deduped_in_first_reference_order() {
        // One warning per name, not per occurrence. Deduped the way lookup matches, so a name
        // referenced in two casings is one finding.
        let expansion =
            expand("$(Second)$(First)$(SECOND)$(First)", &Properties::new()).expect("expands");

        assert_eq!(expansion.value, "");
        assert_eq!(expansion.undefined, ["Second", "First"]);
    }

    #[test]
    fn item_and_metadata_sigils_are_left_alone() {
        // `%(...)` inheritance is MVP-12's and item lists are out of scope entirely. `$` is the
        // only sigil this pass knows, and touching the others would corrupt a value MVP-12 has to
        // read intact.
        let props = bag(&[("ProjectDir", r"C:\proj\")]);
        let expansion = expand(
            r"$(ProjectDir)inc;%(AdditionalIncludeDirectories);@(Sources)",
            &props,
        )
        .expect("expands");

        assert_eq!(
            expansion.value,
            r"C:\proj\inc;%(AdditionalIncludeDirectories);@(Sources)"
        );
        assert!(expansion.undefined.is_empty());
        assert!(expansion.unsupported.is_empty());
    }

    #[test]
    fn a_dollar_outside_a_reference_is_literal() {
        // Only `$(` opens a reference. MSBuild spells a literal `$` as `%24`, so a bare one in a
        // value is just text and must survive as text.
        let props = bag(&[("A", "1")]);

        for input in ["$", "$x", "100$", "a$b", "$)", "$["] {
            assert_eq!(expanded(input, &props), input, "for {input:?}");
        }

        assert_eq!(
            expanded("$$(A)", &props),
            "$1",
            "the first `$` is literal and the second opens the reference"
        );
    }

    #[test]
    fn a_construct_outside_the_supported_subset_is_recorded_and_left_verbatim() {
        // Emptying these would silently drop an include directory and surface as a missing header
        // with nothing to trace it to. Left as written, the bogus entry in the database names its
        // own cause — and the recording is what makes MVP-13 able to say so up front.
        let cases = [
            (
                "$([System.IO.Path]::Combine('a','b'))",
                "a property function",
            ),
            ("$(Foo.Substring(0))", "a method call on a property"),
            (
                r"$(Registry:HKEY_LOCAL_MACHINE\Software\X@Version)",
                "a registry read",
            ),
            ("$()", "an empty reference"),
        ];

        for (input, what) in cases {
            let expansion = expand(input, &Properties::new()).expect("expands anyway");

            assert_eq!(expansion.value, input, "for {what}");
            assert_eq!(expansion.unsupported, [input], "for {what}");
            assert!(
                expansion.undefined.is_empty(),
                "for {what}: an unsupported construct is not an undefined property"
            );
        }
    }

    #[test]
    fn a_construct_keeps_its_own_parentheses() {
        // The reason the close is found by a depth-tracking scan rather than `find(')')`. A naive
        // scan would end this construct at the `)` closing `Combine(`, leave a stray `)` in the
        // output, and quote half an expression in the diagnostic.
        let expansion = expand(
            "$([System.IO.Path]::Combine('a','b'));inc",
            &Properties::new(),
        )
        .expect("expands");

        assert_eq!(expansion.value, "$([System.IO.Path]::Combine('a','b'));inc");
        assert_eq!(
            expansion.unsupported,
            ["$([System.IO.Path]::Combine('a','b'))"]
        );
    }

    #[test]
    fn a_reference_inside_an_unsupported_construct_is_part_of_it() {
        // The construct is recorded and copied whole, so the `$(Root)` inside it is *not* a
        // lookup. Expanding it would produce a half-processed expression that is neither what the
        // file says nor anything MSBuild would produce.
        let props = bag(&[("Root", r"C:\proj")]);
        let input = r"$([System.IO.Path]::Combine($(Root),'inc'));inc";
        let expansion = expand(input, &props).expect("expands");

        assert_eq!(expansion.value, input);
        assert_eq!(
            expansion.unsupported,
            [r"$([System.IO.Path]::Combine($(Root),'inc'))"]
        );
    }

    #[test]
    fn a_nested_reference_is_recorded_as_unsupported() {
        // `$($(Inner))` builds a property name out of another property. Legal nowhere in the
        // supported subset, and resolving it by accident would be a guess.
        let props = bag(&[("Inner", "Outer"), ("Outer", "value")]);
        let expansion = expand("$($(Inner))", &props).expect("expands");

        assert_eq!(expansion.value, "$($(Inner))");
        assert_eq!(expansion.unsupported, ["$($(Inner))"]);
    }

    #[test]
    fn an_unterminated_reference_is_recorded_as_unsupported() {
        // MSBuild fails the build outright. Recorded and copied through instead: one stray `$(`
        // in a property nothing reads should not sink a whole generation when there is a warning
        // channel to say so. Everything before it still expands.
        let props = bag(&[("A", "1")]);
        let expansion = expand("$(A);$(Unterminated", &props).expect("expands anyway");

        assert_eq!(expansion.value, "1;$(Unterminated");
        assert_eq!(expansion.unsupported, ["$(Unterminated"]);
        assert!(expansion.undefined.is_empty());
    }

    #[test]
    fn unsupported_constructs_are_deduped() {
        let expansion = expand("$(A.B);$(A.B)", &Properties::new()).expect("expands");

        assert_eq!(expansion.unsupported, ["$(A.B)"]);
    }

    #[test]
    fn assign_hands_back_what_it_could_not_resolve() {
        // The caller has the file and the line; this module has neither. Returning the recording
        // rather than warning is what keeps the wording MVP-13's to own.
        let mut props = Properties::new();
        let expansion = props
            .assign("IncludePath", "$(Missing)inc")
            .expect("expands");

        assert_eq!(expansion.value, "inc");
        assert_eq!(expansion.undefined, ["Missing"]);
        assert_eq!(
            props.get("IncludePath"),
            Some("inc"),
            "the expanded value is what gets stored, never the raw one"
        );
    }

    #[test]
    fn a_value_exactly_at_the_cap_is_allowed() {
        // The cap is a ceiling, not a limit below it. A real include-directory list runs to a few
        // kilobytes, so nothing legitimate comes close — but an off-by-one here would reject a
        // file for no reason a user could diagnose.
        let big = "x".repeat(MAX_VALUE_BYTES);
        let props = bag(&[("A", &big)]);

        let expansion = expand("$(A)", &props).expect("expands");

        assert_eq!(expansion.value.len(), MAX_VALUE_BYTES);
    }

    /// The guard MVP-08's "cyclic reference erroring at the depth limit" test turns into.
    ///
    /// Eager expansion makes a cycle impossible to construct, but not runaway growth: this doubles
    /// the value on every assignment, so forty rounds ask for roughly a terabyte. The cap stops
    /// it, and `assign` is what has to stop it — a single `expand` never sees the growth.
    #[test]
    fn a_runaway_self_reference_fails_at_the_size_cap() {
        let mut props = Properties::new();
        props.set("A", "x");

        let error = (0..40)
            .try_for_each(|_| props.assign("A", "$(A)$(A)").map(drop))
            .expect_err("must not be allowed to double indefinitely");

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn a_single_oversized_input_is_refused() {
        // No self-reference required: a hostile project file can simply carry an enormous value,
        // and the check after the loop is what catches it.
        let huge = "x".repeat(MAX_VALUE_BYTES + 1);

        let error = expand(&huge, &Properties::new()).expect_err("must be refused");

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn the_size_cap_error_names_the_value_without_reprinting_it() {
        // The input that trips the cap is by definition huge. A message that quoted it in full
        // would be unreadable in a terminal and useless in the JSON envelope.
        let mut props = Properties::new();
        props.set("A", "x".repeat(MAX_VALUE_BYTES));

        let error = expand("$(A)$(A)", &props).expect_err("must be refused");

        let message = error.to_string();
        assert!(message.contains("$(A)$(A)"), "names the value: {message}");
        assert!(
            message.len() < 200,
            "must not reprint the value: {} bytes",
            message.len()
        );
    }
}
