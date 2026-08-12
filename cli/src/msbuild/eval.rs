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

use std::{collections::HashMap, collections::HashSet, path::Path};

use crate::{Error, Reporter, Result};

/// How large one expanded value may grow before the expansion is refused.
///
/// Eager expansion makes cycles impossible, but not runaway growth.
const MAX_VALUE_BYTES: usize = 1 << 20;

/// How deeply a condition may nest before it is refused. Real conditions rarely nest more than 3 or
/// 4 deep, so this is very generous
const MAX_CONDITION_DEPTH: usize = 32;

/// The property bag built up during evaluation. Later assignments shadow earlier ones.
#[derive(Debug, Default, Clone)]
pub struct Properties {
    values: HashMap<String, String>,
    /// Keys that cannot be overwritten by project configuration data.
    locked: HashSet<String>,
    /// Keys whose *stored* value still carries an unsupported construct copied through verbatim
    /// by [`expand`] -- e.g. `<_PlatformFolder>$(VCTargetsPath)$([MSBuild]::Unescape(...))</_...>`.
    /// Tracked so a later `$(_PlatformFolder)` reference is itself flagged unsupported, even though
    /// by itself it is an ordinary, well-formed property reference. Without this, one hop through
    /// an intermediate property is enough to make an untrustworthy value look clean again -- which
    /// is exactly how the real `Microsoft.Cpp.ToolsetLocation.props` reaches `<Import>`.
    tainted: HashSet<String>,
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

    /// Store `value` as given, shadowing any earlier assignment — **including a reserved one**.
    ///
    /// Raw, because `msbuild/reserved.rs` seeds reserved properties whose values are literal paths
    /// with nothing to expand. Anything read out a project file goes through
    /// [`Properties::assign`], which is where the lock is enforced: the trust boundary is which
    /// function the caller reaches for, not which key it names. That asymmetry is load-bearing
    /// rather than an oversight — `MSBuildThisFile*` is reserved against project files and still has
    /// to be rewritten on every file the import walk enters, and restored on the way back out.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.values.insert(name.to_ascii_lowercase(), value.into());
    }

    /// Store `value` and refuse any later [`Properties::assign`] to `name`.
    ///
    /// MSBuild reserves its own `MSBuild*` properties outright and treats the ones a solution
    /// provides (`Configuration`, `Platform`, `Solution*`) as global, which a project file also
    /// cannot override.
    pub fn set_reserved(&mut self, name: &str, value: impl Into<String>) {
        let key = name.to_ascii_lowercase();
        self.values.insert(key.clone(), value.into());
        self.locked.insert(key);
    }

    /// Whether `name` is considered reserved and cannot be assigned to
    pub fn is_reserved(&self, name: &str) -> bool {
        self.locked.contains(&name.to_ascii_lowercase())
    }

    /// Whether `name`'s stored value still carries an unsupported construct, copied through
    /// verbatim by an earlier [`Properties::assign`]. See the field doc on `tainted`.
    pub fn is_tainted(&self, name: &str) -> bool {
        self.tainted.contains(&name.to_ascii_lowercase())
    }

    /// Expand `value` against the contents of the bag at this moment, then store the result.
    ///
    /// The returned [`Expansion`] carries what could not be resolved, for the caller to report
    /// warnings with.
    pub fn assign(&mut self, name: &str, value: &str) -> Result<Expansion> {
        let mut expansion = expand(value, self)?;

        if self.is_reserved(name) {
            expansion.refused = true;
            return Ok(expansion);
        }

        let key = name.to_ascii_lowercase();
        if expansion.unsupported.is_empty() {
            self.tainted.remove(&key);
        } else {
            self.tainted.insert(key);
        }
        self.set(name, expansion.value.clone());
        Ok(expansion)
    }
}

/// Item metadata for one item, keyed case-insensitively.
///
/// The `%(Name)` counterpart to [`Properties`], and a separate type rather than a second bag within
/// the same one. This is because Metadata is scoped to an *item* rather than to the evaluation, so
/// an item takes a clone of the `ItemDefinitionGroup` defaults at the time of its declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    values: HashMap<String, String>,
}

impl Metadata {
    pub fn new() -> Self {
        Metadata::default()
    }

    /// `Some("")` and `None` stay distinct, as they do in [`Properties::get`].
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.values.insert(name.to_ascii_lowercase(), value.into());
    }

    /// Expand `value` -- `$()` against `props`, then `%()` against self and store the result in self.
    ///
    /// Eager, in the same way as [`Properties::assign`], so what is stored should contain no sigils
    /// and no cycle can be constructed. `$` parses first because a property's value was already
    /// expanded when it was assigned and so holds no `%(`.
    pub fn assign(&mut self, name: &str, value: &str, props: &Properties) -> Result<Expansion> {
        let mut expansion = expand(value, props)?;
        let metadata = expand_metadata(&expansion.value, self)?;

        expansion.value = metadata.value;
        for reference in metadata.unsupported {
            expansion.record_unsupported(&reference);
        }

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
    /// Set when the assignment target was reserved. `value` is the expansion that would have been
    /// assigned.
    pub refused: bool,
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

    /// Report what this expansion could not resolve, and hand back the value.
    ///
    /// Consuming, so the caller cannot report the same finding twice.
    pub fn report(self, reporter: &mut Reporter, file: &Path, line: usize) -> String {
        report(reporter, self.undefined, self.unsupported, file, line);
        self.value
    }
}

/// Turn the findings an expansion or condition produced into a warning diagnostic.
fn report(
    reporter: &mut Reporter,
    undefined: Vec<String>,
    unsupported: Vec<String>,
    file: &Path,
    line: usize,
) {
    for name in undefined {
        reporter.warn_at(
            file,
            Some(line),
            format!("`$({name})` is not defined and expanded to nothing"),
        );
    }

    for reference in unsupported {
        reporter.warn_at(
            file,
            Some(line),
            format!("`{reference}` is not supported and was left as written"),
        );
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
                Some(value) => {
                    out.value.push_str(value);
                    // The reference itself is well-formed; what it resolves to is not. Recording
                    // it here is what lets an unsupported construct survive being copied through
                    // an intermediate property rather than looking clean again one hop later.
                    if props.is_tainted(body) {
                        out.record_unsupported(reference);
                    }
                }
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

/// Substitute `%(Name)` references in `input` against `metadata` bag.
///
/// A name the bag has no value for expands to **nothing and is not recorded**. That is the base for
/// the whole inheritance mechanism. The first `ItemDefinitionGroup` in a graph writes
/// `WIN32;%(PreprocessorDefinitions)` against an empty bag, so recording it would warn on every
/// idiomatic project file.
///
/// Qualified references (`%(ClCompile.Identity)`), item transforms (`@(...)`) and an unterminated
/// `%(` are copied through verbatim and recorded, as [`expand`] does for the `$` equivalents.
/// `%XX` character escapes are not decoded and not recorded.
pub fn expand_metadata(input: &str, metadata: &Metadata) -> Result<Expansion> {
    let mut out = Expansion::default();
    let mut rest = input;

    while let Some(open) = next_sigil(rest) {
        out.value.push_str(&rest[..open]);
        let body_start = open + 2;

        let Some(offset) = closing_paren(&rest[body_start..]) else {
            out.record_unsupported(&rest[open..]);
            out.value.push_str(&rest[open..]);
            rest = "";
            break;
        };

        let close = body_start + offset;
        let body = &rest[body_start..close];
        let reference = &rest[open..close + 1];

        if rest.as_bytes()[open] == b'@' || !is_property_name(body) {
            out.record_unsupported(reference);
            out.value.push_str(reference);
        } else if let Some(value) = metadata.get(body) {
            out.value.push_str(value);
        }

        check_size(&out.value, input)?;
        rest = &rest[close + 1..];
    }

    out.value.push_str(rest);
    check_size(&out.value, input)?;
    Ok(out)
}

/// The offset of the next `%(` or `@(`, whichever comes first.
fn next_sigil(text: &str) -> Option<usize> {
    match (text.find("%("), text.find("@(")) {
        (Some(metadata), Some(item)) => Some(metadata.min(item)),
        (found, None) | (None, found) => found,
    }
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

/// Whether `value` ends in a path separator, as MSBuild's `HasTrailingSlash` condition function
/// tests. Both separators count -- `.vcxproj` paths mix `\` (the toolchain's own convention) and
/// `/` (globs, wildcard specs) freely, and MSBuild itself checks both.
fn has_trailing_slash(value: &str) -> bool {
    value.ends_with('/') || value.ends_with('\\')
}

// Shorten a value for a message.
fn truncate(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}...", &input[..cut]),
        None => input.to_string(),
    }
}

/// One evaluated `Condition`, and what could not be resolved while evaluating it.
///
/// Mirrors [`Expansion`] and for the same reason. A *syntax* this evaluator doesn't support is an
/// error rather than a finding -- there is no truth value to hand back. Whereas an unsupported
/// `$(..)` inside an operand lands in `unsupported` and still yields an answer because MVP-08 left
/// it verbatim and there are two strings to compare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[must_use]
pub struct Evaluation {
    pub holds: bool,
    /// Merged across every operand, expanded, deduped and in first-reference order.
    pub undefined: Vec<String>,
    pub unsupported: Vec<String>,
}

impl Evaluation {
    /// Take one operand's findings and hand back its expanded text.
    fn absorb(&mut self, expansion: Expansion) -> String {
        for name in expansion.undefined {
            if !self
                .undefined
                .iter()
                .any(|seen| seen.eq_ignore_ascii_case(&name))
            {
                self.undefined.push(name);
            }
        }

        for reference in expansion.unsupported {
            if !self.unsupported.contains(&reference) {
                self.unsupported.push(reference);
            }
        }

        expansion.value
    }

    /// Report what this condition could not resolve, and hand back whether it holds.
    ///
    /// Consuming, so the caller cannot report the same finding twice.
    pub fn report(self, reporter: &mut Reporter, file: &Path, line: usize) -> bool {
        report(reporter, self.undefined, self.unsupported, file, line);
        self.holds
    }
}

/// Evaluate an MSBuild `Condition` expression as a boolean.
///
/// `file` names the file the condition came from.
///
/// An empty condition holds, so does an absent one
pub fn condition_holds(
    condition: &str,
    props: &Properties,
    file: &Path,
    line: usize,
) -> Result<Evaluation> {
    if condition.trim().is_empty() {
        return Ok(Evaluation {
            holds: true,
            ..Evaluation::default()
        });
    }

    let tokens = tokenize(condition, file, line)?;

    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        depth: 0,
        props,
        file,
        line,
        condition,
        found: Evaluation::default(),
    };

    let holds = parser.expression()?;
    if parser.pos != tokens.len() {
        // Trailing tokens: `'a' =='b' 'c'`. Well-formed to a point and then not.
        return Err(parser.unsupported());
    }

    let mut evaluation = parser.found;
    evaluation.holds = holds;
    Ok(evaluation)
}

/// One lexical unit of a condition.
///
/// `Quoted` and `Bare` both hold text that is still unexpanded.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Not,
    Eq,
    Ne,
    And,
    Or,
    Quoted(String),
    Bare(String),
}

fn tokenize(condition: &str, file: &Path, line: usize) -> Result<Vec<Token>> {
    let unsupported = || Error::UnsupportedConstruct {
        file: file.to_path_buf(),
        line: Some(line),
        construct: truncate(condition, 80),
    };

    let mut tokens = Vec::new();
    let bytes = condition.as_bytes();
    let mut at = 0;

    while at < condition.len() {
        let rest = &condition[at..];

        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }

        // Prevent `!=` and `==` from being confused as 2 single-byte entities
        if let Some(token) = rest
            .strip_prefix("==")
            .map(|_| Token::Eq)
            .or_else(|| rest.strip_prefix("!=").map(|_| Token::Ne))
        {
            tokens.push(token);
            at += 2;
            continue;
        }

        match bytes[at] {
            b'(' => {
                tokens.push(Token::Open);
                at += 1;
                continue;
            }
            b')' => {
                tokens.push(Token::Close);
                at += 1;
                continue;
            }
            b'!' => {
                tokens.push(Token::Not);
                at += 1;
                continue;
            }
            b'\'' => {
                let Some(end) = rest[1..].find('\'') else {
                    return Err(unsupported());
                };
                tokens.push(Token::Quoted(rest[1..1 + end].to_string()));
                at += end + 2;
                continue;
            }
            _ => {}
        }

        let len = bare_token_len(rest);
        if len == 0 {
            // Some character we don't model: '<', '>', ',', '%'.
            return Err(unsupported());
        }

        let text = &rest[..len];
        tokens.push(if text.eq_ignore_ascii_case("and") {
            Token::And
        } else if text.eq_ignore_ascii_case("or") {
            Token::Or
        } else {
            Token::Bare(text.to_string())
        });
        at += len;
    }

    Ok(tokens)
}

/// How many bytes of `rest` form one bare token.
///
/// A `$(...)` group is skipped whole, so the `)` inside `$(Configuration)` does not end the token.
/// Without that the reference would tokenize as three things and every unquoted operand would be
/// unparseable.
fn bare_token_len(rest: &str) -> usize {
    let mut at = 0;

    while at < rest.len() {
        if rest[at..].starts_with("$(") {
            match closing_paren(&rest[at + 2..]) {
                Some(offset) => {
                    at += 2 + offset + 1;
                    continue;
                }
                // Unterminated. Taken whole so MVP-08 records it when the operand is expanded.
                None => return rest.len(),
            }
        }

        let Some(c) = rest[at..].chars().next() else {
            break;
        };
        if c.is_whitespace() || matches!(c, '(' | ')' | '\'' | '!' | '=') {
            break;
        }

        at += c.len_utf8();
    }

    at
}

/// Recursively descends through the token stream, evaluating as it goes.
///
/// Doesn't bother with an AST, as it's a simple grammar.
///
/// The grammar for conditional expressions from MSBuild is:
/// condition := or
/// or        := and ( 'Or' and )*
/// and       := unary ( 'And' unary )*
/// unary     := '!' unary | primary
/// primary   := '(' condition ')'
///            | 'Exists' '(' operand ')'
///            | 'HasTrailingSlash' '(' operand ')'
///            | operand ( ('==' | '!=') operand )?     -- a lone operand is a boolean literal
/// operand   := "'" text "'" | bare-token
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
    props: &'a Properties,
    file: &'a Path,
    line: usize,
    condition: &'a str,
    found: Evaluation,
}

impl Parser<'_> {
    /// `or := and ( 'Or' and )*`
    fn expression(&mut self) -> Result<bool> {
        let mut value = self.conjunction()?;
        while self.take(&Token::Or) {
            // Deliberately not short-circuiting. Evaluating the right side no matter what allows for
            // bad constructs to get reported
            let right = self.conjunction()?;
            value = value || right;
        }
        Ok(value)
    }

    /// `and := unary ( 'And' unary )*` -- Binds tighter than 'Or'
    fn conjunction(&mut self) -> Result<bool> {
        let mut value = self.unary()?;
        while self.take(&Token::And) {
            let right = self.unary()?;
            value = value && right;
        }
        Ok(value)
    }

    /// `unary := '!' unary | primary`
    fn unary(&mut self) -> Result<bool> {
        if self.take(&Token::Not) {
            return Ok(!self.deeper(Self::unary)?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<bool> {
        match self.bump()? {
            Token::Open => {
                let value = self.deeper(Self::expression)?;
                if !self.take(&Token::Close) {
                    return Err(self.unsupported());
                }
                Ok(value)
            }
            Token::Bare(name) if name.eq_ignore_ascii_case("Exists") => {
                let path = self.single_operand()?;
                Ok(self.exists(&path))
            }
            Token::Bare(name) if name.eq_ignore_ascii_case("HasTrailingSlash") => {
                let value = self.single_operand()?;
                Ok(has_trailing_slash(&value))
            }
            token @ (Token::Quoted(_) | Token::Bare(_)) => self.comparison(token),
            _ => Err(self.unsupported()),
        }
    }

    /// `operand ( ('==' | '!=') operand )?`
    fn comparison(&mut self, left: Token) -> Result<bool> {
        let left = self.operand(left)?;

        let equals = if self.take(&Token::Eq) {
            true
        } else if self.take(&Token::Ne) {
            false
        } else {
            // A lone operand is a boolean: `Condition="$(BuildProjectReferences)"`.
            return self.boolean(&left);
        };

        let token = self.bump()?;
        let right = self.operand(token)?;

        // MSBuild compares conditional strings case-insensitively.
        Ok(left.eq_ignore_ascii_case(&right) == equals)
    }

    /// `'(' operand ')'` -- the argument list every single-operand condition function shares.
    fn single_operand(&mut self) -> Result<String> {
        if !self.take(&Token::Open) {
            return Err(self.unsupported());
        }
        let token = self.bump()?;
        let value = self.operand(token)?;
        if !self.take(&Token::Close) {
            return Err(self.unsupported());
        }
        Ok(value)
    }

    /// Whether `path` names something on disk, relative to the directory of the file the
    /// condition is written in -- as MSBuild resolves it, not the process working directory.
    /// Existence only check. No reading or executing.
    fn exists(&self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() {
            // `Exists('$(Undefined)')` is false, not an error. It is how property sheets ask
            // whether an optional path was configured at all.
            return false;
        }

        let base = self.file.parent().unwrap_or(Path::new(""));
        base.join(path).exists()
    }

    /// Expand an operand and take its findings.
    fn operand(&mut self, token: Token) -> Result<String> {
        let raw = match token {
            Token::Quoted(text) | Token::Bare(text) => text,
            _ => return Err(self.unsupported()),
        };
        let expansion = expand(&raw, self.props)?;
        Ok(self.found.absorb(expansion))
    }

    fn boolean(&mut self, value: &str) -> Result<bool> {
        if value.eq_ignore_ascii_case("true") {
            return Ok(true);
        }
        if value.eq_ignore_ascii_case("false") {
            return Ok(false);
        }
        Err(self.unsupported())
    }

    fn take(&mut self, expected: &Token) -> bool {
        if self.tokens.get(self.pos) == Some(expected) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn bump(&mut self) -> Result<Token> {
        let token = self
            .tokens
            .get(self.pos)
            .ok_or_else(|| self.unsupported())?;
        self.pos += 1;
        Ok(token.clone())
    }

    /// Recurse one lvel, refusing to go deeper than [`MAX_CONDITION_DEPTH`]
    fn deeper<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.depth += 1;
        if self.depth > MAX_CONDITION_DEPTH {
            return Err(Error::RecursionLimit {
                limit: MAX_CONDITION_DEPTH,
                context: format!(
                    "evaluating the condition `{}`",
                    truncate(self.condition, 60)
                ),
            });
        }

        let value = parse(self);
        self.depth -= 1;
        value
    }

    /// The whole condition is named, rather than just the offending token. Otherwise it is hard for
    /// a user to trace back where it originated.
    fn unsupported(&self) -> Error {
        Error::UnsupportedConstruct {
            file: self.file.to_path_buf(),
            line: Some(self.line),
            construct: truncate(self.condition, 80),
        }
    }
}
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{ErrorKind, Reporter, Severity};

    /// A bag seeded verbatim — the shape `msbuild/reserved.rs` hands over, minus the lock.
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

    // ---------------------------------------------------------------------------------------
    // MVP-36 — taint propagation through an intermediate property
    // ---------------------------------------------------------------------------------------

    #[test]
    fn a_property_assigned_from_an_unsupported_construct_is_tainted() {
        let mut props = Properties::new();

        let expansion = props
            .assign("Folder", "$([System.IO.Path]::GetTempPath())")
            .expect("expands anyway");

        assert_eq!(
            expansion.unsupported,
            ["$([System.IO.Path]::GetTempPath())"]
        );
        assert!(props.is_tainted("Folder"));
        assert!(props.is_tainted("folder"), "matched case-insensitively");
    }

    #[test]
    fn referencing_a_tainted_property_is_itself_recorded_as_unsupported() {
        // The gap the real `Microsoft.Cpp.ToolsetLocation.props` exposed: a value built from an
        // unsupported construct is stored as literal, garbled text (ADR 08). One hop later,
        // `$(Folder)` is a perfectly well-formed property reference — nothing about *this*
        // expansion looks unsupported unless the bag remembers where the value came from.
        let mut props = Properties::new();
        assign(&mut props, "Folder", "$([System.IO.Path]::GetTempPath())");

        let expansion = expand("$(Folder)Platform.props", &props).expect("expands anyway");

        assert_eq!(
            expansion.value, "$([System.IO.Path]::GetTempPath())Platform.props",
            "the tainted value's literal text still comes through, same as any other reference"
        );
        assert_eq!(expansion.unsupported, ["$(Folder)"]);
        assert!(expansion.undefined.is_empty());
    }

    #[test]
    fn taint_is_transitive_across_several_hops() {
        let mut props = Properties::new();
        assign(&mut props, "Inner", "$([Fake]::Root())");
        assign(&mut props, "Outer", "$(Inner)\\sub");

        assert!(props.is_tainted("Outer"), "taint propagates through assign");

        let expansion = expand("$(Outer)\\Platform.props", &props).expect("expands anyway");
        assert_eq!(expansion.unsupported, ["$(Outer)"]);
    }

    #[test]
    fn reassigning_a_tainted_property_from_a_clean_value_clears_the_taint() {
        let mut props = Properties::new();
        assign(&mut props, "Folder", "$([Fake]::Root())");
        assert!(props.is_tainted("Folder"));

        assign(&mut props, "Folder", "clean");

        assert!(!props.is_tainted("Folder"));
        let expansion = expand("$(Folder)", &props).expect("expands");
        assert!(expansion.unsupported.is_empty());
    }

    #[test]
    fn a_property_seeded_raw_is_never_tainted() {
        // `set`/`set_reserved` are the trusted-seed path (MVP-11): a reserved property's literal
        // value may itself contain `$(` characters with nothing to expand, and that must not read
        // as "unsupported" the way a project file's own unresolved construct does.
        let mut props = Properties::new();
        props.set_reserved("MSBuildThisFileFullPath", r"C:\proj\$literal$\app.vcxproj");

        assert!(!props.is_tainted("MSBuildThisFileFullPath"));
        let expansion = expand("$(MSBuildThisFileFullPath)", &props).expect("expands");
        assert!(expansion.unsupported.is_empty());
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

    // ---------------------------------------------------------------------------------------
    // MVP-11 — reserved keys
    // ---------------------------------------------------------------------------------------

    #[test]
    fn a_reserved_key_refuses_an_assignment_and_keeps_its_value() {
        // The mechanism half of MVP-11. `msbuild/reserved.rs` decides *which* names are reserved;
        // this decides what being reserved does. Enforcing it here rather than at the one call site
        // in `imports.rs` is what makes it a guarantee instead of a convention — the bag cannot be
        // talked out of it by a later caller that forgets to ask.
        let mut props = Properties::new();
        props.set_reserved("MSBuildProjectDirectory", r"C:\proj\app");

        let expansion = props
            .assign("MSBuildProjectDirectory", "hijacked")
            .expect("expands");

        assert!(expansion.refused);
        assert_eq!(props.get("MSBuildProjectDirectory"), Some(r"C:\proj\app"));
    }

    #[test]
    fn a_refused_assignment_still_reports_what_it_could_not_resolve() {
        // The value is expanded even though it is thrown away, for the reason `And`/`Or` do not
        // short-circuit: a construct nobody ends up using is still a construct this tool does not
        // understand, and a user tracing a bad include path needs to hear about it.
        let mut props = Properties::new();
        props.set_reserved("Configuration", "Debug");

        let expansion = props
            .assign(
                "Configuration",
                "$(Missing)$([System.IO.Path]::Combine('a'))",
            )
            .expect("expands");

        assert!(expansion.refused);
        assert_eq!(expansion.undefined, ["Missing"]);
        assert_eq!(expansion.unsupported, ["$([System.IO.Path]::Combine('a'))"]);
        assert_eq!(
            props.get("Configuration"),
            Some("Debug"),
            "the expansion is reported, not stored"
        );
    }

    #[test]
    fn an_ordinary_assignment_is_not_refused() {
        // The negative case, so `refused` cannot be satisfied by always setting it.
        let mut props = Properties::new();
        props.set_reserved("ProjectDir", r"C:\proj\app\");

        let expansion = props
            .assign("IncludePath", "$(ProjectDir)inc")
            .expect("expands");

        assert!(!expansion.refused);
        assert_eq!(props.get("IncludePath"), Some(r"C:\proj\app\inc"));
    }

    #[test]
    fn set_can_still_overwrite_a_reserved_key() {
        // What the import walk depends on: `MSBuildThisFile*` is reserved against project files and
        // rewritten by the walk on every file it enters. The trust boundary is which *function* is
        // called — `set` is seeding, `assign` is a project file talking — not which key it names.
        let mut props = Properties::new();
        props.set_reserved("MSBuildThisFile", "Lib.vcxproj");

        props.set("MSBuildThisFile", "Common.props");
        assert_eq!(props.get("MSBuildThisFile"), Some("Common.props"));

        props.set_reserved("MSBuildThisFile", "Lib.vcxproj");
        assert_eq!(props.get("MSBuildThisFile"), Some("Lib.vcxproj"));
    }

    #[test]
    fn the_reserved_lock_is_matched_case_insensitively() {
        // Anything less would make the lock trivially avoidable by a project file writing
        // `<solutiondir>`, which MSBuild resolves to the same property.
        let mut props = Properties::new();
        props.set_reserved("SolutionDir", r"C:\sln\");

        assert!(props.is_reserved("solutiondir"));
        assert!(props.is_reserved("SOLUTIONDIR"));

        let expansion = props.assign("solutiondir", "hijacked").expect("expands");

        assert!(expansion.refused);
        assert_eq!(props.get("SolutionDir"), Some(r"C:\sln\"));
    }

    #[test]
    fn nothing_is_reserved_in_a_bag_that_was_never_seeded() {
        // Seeding must not lock the bag itself. An empty bag is still a valid starting point for
        // `imports::resolve`, and everything a project file defines for its own use is ordinary.
        let mut props = bag(&[("PreprocessorDefinitions", "WIN32")]);

        assert!(!props.is_reserved("PreprocessorDefinitions"));
        assert!(!props.is_reserved("Configuration"));

        let expansion = props
            .assign(
                "PreprocessorDefinitions",
                "$(PreprocessorDefinitions);_DEBUG",
            )
            .expect("expands");

        assert!(!expansion.refused);
        assert_eq!(props.get("PreprocessorDefinitions"), Some("WIN32;_DEBUG"));
    }

    // ---------------------------------------------------------------------------------------
    // MVP-09 — `Condition` evaluation
    // ---------------------------------------------------------------------------------------

    /// A condition site inside the crate directory.
    ///
    /// `Exists()` resolves against the directory of the file the condition is written in, so the
    /// site has to be a real place for the `Exists()` cases to mean anything. The crate directory
    /// is one that exists on every machine able to run this suite — which is why `Exists()` needs
    /// no injected filesystem the way toolchain discovery needs `FakeToolchain`: real Visual
    /// Studio paths differ per machine, and `Cargo.toml` does not.
    fn site() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("app.vcxproj")
    }

    /// Evaluate against `props`, asserting nothing was recorded — what most cases want.
    fn holds(condition: &str, props: &Properties) -> bool {
        let evaluation = condition_holds(condition, props, &site(), 12).expect("evaluates");
        assert!(
            evaluation.undefined.is_empty(),
            "unexpected undefined: {:?}",
            evaluation.undefined
        );
        assert!(
            evaluation.unsupported.is_empty(),
            "unexpected unsupported: {:?}",
            evaluation.unsupported
        );
        evaluation.holds
    }

    /// Evaluate against an empty bag, for the many cases with no property in them at all.
    fn holds_bare(condition: &str) -> bool {
        holds(condition, &Properties::new())
    }

    /// The error from a condition that must not evaluate.
    fn refuse(condition: &str) -> Error {
        condition_holds(condition, &Properties::new(), &site(), 12).expect_err("must not evaluate")
    }

    #[test]
    fn an_empty_condition_holds() {
        // MSBuild treats an empty `Condition` as an absent one. MVP-07 hands `Condition=""`
        // through as `Some("")`, so this is reachable — and defaulting it to false would silently
        // drop a `PropertyGroup` that applies unconditionally.
        assert!(holds_bare(""));
        assert!(holds_bare("   "));
    }

    #[test]
    fn equality_compares_expanded_operands() {
        // The condition on every configuration-gated group Visual Studio writes.
        let props = bag(&[("Configuration", "Debug"), ("Platform", "x64")]);

        assert!(holds("'$(Configuration)|$(Platform)'=='Debug|x64'", &props));
        assert!(!holds(
            "'$(Configuration)|$(Platform)'=='Release|x64'",
            &props
        ));
    }

    #[test]
    fn inequality_is_the_negation_of_equality() {
        let props = bag(&[("Configuration", "Debug")]);

        assert!(holds("'$(Configuration)' != 'Release'", &props));
        assert!(!holds("'$(Configuration)' != 'Debug'", &props));
    }

    #[test]
    fn comparison_is_case_insensitive() {
        // MSBuild compares condition strings without regard to case. Comparing exactly would make
        // `'$(Configuration)'=='debug'` skip a group that MSBuild applies — a database missing
        // flags, with nothing saying so.
        let props = bag(&[("Configuration", "Debug")]);

        assert!(holds_bare("'Debug'=='debug'"));
        assert!(holds("'$(Configuration)'=='DEBUG'", &props));
        assert!(!holds("'$(Configuration)' != 'dEbUg'", &props));
    }

    #[test]
    fn an_unquoted_operand_is_compared_the_same_way() {
        // Quotes are conventional, not required. This is also the case that pins the tokenizer:
        // `$(Platform)` contains a `)`, and a scanner that ended a bare token there would split
        // one operand into three tokens and fail to parse.
        let props = bag(&[("Platform", "x64")]);

        assert!(holds("$(Platform)==x64", &props));
        assert!(!holds("$(Platform)==Win32", &props));
    }

    #[test]
    fn an_undefined_operand_compares_as_empty_and_is_recorded() {
        // `'$(X)' != ''` is how a property sheet tests for a property it expects to be absent, so
        // this path is ordinary rather than exceptional — but a *typo* looks identical, which is
        // why the name is recorded for MVP-13 rather than thrown away.
        let evaluation =
            condition_holds("'$(Typo)'==''", &Properties::new(), &site(), 12).expect("evaluates");

        assert!(evaluation.holds);
        assert_eq!(evaluation.undefined, ["Typo"]);
        assert!(evaluation.unsupported.is_empty());
    }

    #[test]
    fn and_requires_both_sides() {
        assert!(holds_bare("'a'=='a' And 'b'=='b'"));
        assert!(!holds_bare("'a'=='a' And 'b'=='c'"));
        assert!(!holds_bare("'a'=='z' And 'b'=='b'"));
        assert!(!holds_bare("'a'=='z' And 'b'=='c'"));
    }

    #[test]
    fn or_requires_either_side() {
        assert!(holds_bare("'a'=='a' Or 'b'=='b'"));
        assert!(holds_bare("'a'=='a' Or 'b'=='c'"));
        assert!(holds_bare("'a'=='z' Or 'b'=='b'"));
        assert!(!holds_bare("'a'=='z' Or 'b'=='c'"));
    }

    #[test]
    fn not_inverts_its_operand() {
        assert!(holds_bare("!('a'=='b')"));
        assert!(!holds_bare("!('a'=='a')"));
        assert!(holds_bare("!!('a'=='a')"), "stacking is not special-cased");
        assert!(!holds_bare("!'true'"), "a bare boolean can be negated too");
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // Discriminating on purpose. Read as `a Or (b And c)` this is true; read left to right as
        // `(a Or b) And c` it is false, so the assertion fails if precedence is flat.
        assert!(holds_bare("'a'=='a' Or 'b'=='c' And 'd'=='e'"));
    }

    #[test]
    fn parentheses_override_precedence() {
        // The same expression as above, grouped the other way.
        assert!(!holds_bare("('a'=='a' Or 'b'=='c') And 'd'=='e'"));
        assert!(holds_bare("('a'=='a' Or 'b'=='c') And 'd'=='d'"));
    }

    #[test]
    fn operator_keywords_are_case_insensitive() {
        // MSBuild accepts any casing, and real property sheets are inconsistent about it.
        for condition in [
            "'a'=='a' and 'b'=='b'",
            "'a'=='a' AND 'b'=='b'",
            "'a'=='z' or 'b'=='b'",
            "'a'=='z' OR 'b'=='b'",
        ] {
            assert!(holds_bare(condition), "for {condition}");
        }
    }

    #[test]
    fn a_lone_operand_is_a_boolean_literal() {
        // `Condition="$(BuildProjectReferences)"` with no comparison at all.
        let props = bag(&[("Yes", "true"), ("No", "FALSE")]);

        assert!(holds("$(Yes)", &props));
        assert!(!holds("$(No)", &props));
        assert!(holds_bare("'true'"));
        assert!(holds_bare("TRUE"), "matched without regard to case");
        assert!(!holds_bare("false"));
    }

    #[test]
    fn a_lone_operand_that_is_not_a_boolean_is_an_error() {
        // `Condition="$(Configuration)"` where the value is `Debug`. There is no truth value here,
        // and picking one would be a guess.
        assert_eq!(refuse("'Debug'").kind(), ErrorKind::UnsupportedConstruct);
    }

    #[test]
    fn exists_finds_a_file_beside_the_conditions_own_file() {
        // Resolved against the directory of the file the condition is written in, which is what
        // MSBuild does — not against the process working directory, which would make the answer
        // depend on where the user ran the tool from.
        assert!(holds_bare("Exists('Cargo.toml')"));
        assert!(holds_bare("exists('Cargo.toml')"), "the name is a keyword");
    }

    #[test]
    fn exists_resolves_a_subdirectory() {
        // Built with the platform separator: a `\` would be one ordinary filename character on the
        // Linux runner rather than a directory boundary.
        let nested = ["src", "lib.rs"].join(std::path::MAIN_SEPARATOR_STR);

        assert!(holds_bare(&format!("Exists('{nested}')")));
    }

    #[test]
    fn exists_is_false_for_a_missing_path() {
        // False, not an error. A property sheet asking whether an optional file is present is
        // doing exactly what `Exists()` is for.
        assert!(!holds_bare("Exists('no-such-file.props')"));
    }

    #[test]
    fn exists_is_false_for_an_empty_path() {
        // `Exists('$(UserRootDir)\...')` where the property was never set. Every Visual Studio
        // project carries one of these against `Microsoft.Cpp.$(Platform).user.props`.
        let evaluation = condition_holds("Exists('$(Undefined)')", &Properties::new(), &site(), 12)
            .expect("evaluates");

        assert!(!evaluation.holds);
        assert_eq!(evaluation.undefined, ["Undefined"]);
    }

    #[test]
    fn has_trailing_slash_checks_the_last_character() {
        assert!(holds_bare("HasTrailingSlash('C:\\proj\\')"));
        assert!(holds_bare("HasTrailingSlash('C:/proj/')"), "`/` counts too");
        assert!(!holds_bare("HasTrailingSlash('C:\\proj')"));
        assert!(
            holds_bare("hastrailingslash('C:\\proj\\')"),
            "the name is a keyword"
        );
    }

    #[test]
    fn has_trailing_slash_is_false_for_an_empty_operand() {
        // `HasTrailingSlash('$(Undefined)')` is how a property sheet asks whether an optional,
        // possibly-unset path was written with a trailing separator. Empty is not an error.
        let evaluation = condition_holds(
            "HasTrailingSlash('$(Undefined)')",
            &Properties::new(),
            &site(),
            12,
        )
        .expect("evaluates");

        assert!(!evaluation.holds);
        assert_eq!(evaluation.undefined, ["Undefined"]);
    }

    #[test]
    fn has_trailing_slash_can_be_negated() {
        assert!(holds_bare("!HasTrailingSlash('C:\\proj')"));
        assert!(!holds_bare("!HasTrailingSlash('C:\\proj\\')"));
    }

    #[test]
    fn a_quoted_operand_cannot_inject_operators() {
        // The reason the condition is parsed *before* any operand is expanded. Project files are
        // untrusted input (CLAUDE.md §5): expanding first would splice this property's value into
        // the expression, turning one comparison into `'literal' Or 'a'=='a'` and forcing the
        // group to apply. Parsed first, the whole thing is one operand and simply does not match.
        let props = bag(&[("Evil", "literal' Or 'a'=='a"), ("Keyword", "a Or b")]);

        assert!(!holds("'$(Evil)'=='literal'", &props));
        assert!(
            holds("'$(Keyword)'=='a Or b'", &props),
            "a keyword inside an operand's text is text, not an operator"
        );
    }

    #[test]
    fn an_unsupported_operator_is_an_error() {
        // Numeric comparison is outside the subset. Evaluating it as false would silently skip a
        // `PropertyGroup` that applies — the plausible-but-wrong database CLAUDE.md §3 calls the
        // worst outcome — so it fails loudly instead.
        for condition in [
            "'1' < '2'",
            "'$(VisualStudioVersion)' >= '16.0'",
            "'1' <= '2'",
            "'1' > '2'",
        ] {
            assert_eq!(
                refuse(condition).kind(),
                ErrorKind::UnsupportedConstruct,
                "for {condition}"
            );
        }
    }

    #[test]
    fn an_unsupported_function_is_an_error() {
        // `Exists` and `HasTrailingSlash` are the only condition functions in the subset (MVP-42).
        // The version-comparison family is real MSBuild and will turn up the moment MVP-10 starts
        // importing the Visual Studio installation's own `.targets` — which is MVP-36's problem to
        // weigh, not this one's to guess at: those are property functions reached through `$(...)`,
        // a materially larger addition than a second single-operand condition function.
        for condition in [
            "$([MSBuild]::VersionGreaterThanOrEquals('17.0', '16.0'))",
            "HasTrailingSlash('a', 'b')",
        ] {
            assert_eq!(
                refuse(condition).kind(),
                ErrorKind::UnsupportedConstruct,
                "for {condition}"
            );
        }
    }

    #[test]
    fn a_malformed_condition_is_an_error() {
        let cases = [
            ("('a'=='a'", "unclosed parenthesis"),
            ("'a'=='a')", "unopened parenthesis"),
            ("'a'=='a", "unterminated quote"),
            ("'a'=='b' 'c'", "trailing operand"),
            ("()", "empty parentheses"),
            ("'a' ==", "missing right operand"),
            ("And 'a'=='a'", "leading operator"),
            ("'a'=='a' And", "trailing operator"),
        ];

        for (condition, what) in cases {
            assert_eq!(
                refuse(condition).kind(),
                ErrorKind::UnsupportedConstruct,
                "for {what}"
            );
        }
    }

    #[test]
    fn an_unsupported_macro_in_an_operand_is_recorded_not_fatal() {
        // The distinction the design turns on: an unevaluable *expression* has no truth value and
        // errors, while an unevaluable *value* still leaves two strings to compare. MVP-08 left
        // the construct verbatim, so the comparison runs and the finding says the answer may be
        // wrong.
        let evaluation = condition_holds(
            "'$([System.IO.Path]::GetTempPath())'=='C:\\temp\\'",
            &Properties::new(),
            &site(),
            12,
        )
        .expect("evaluates rather than failing");

        assert!(!evaluation.holds);
        assert_eq!(
            evaluation.unsupported,
            ["$([System.IO.Path]::GetTempPath())"]
        );
    }

    #[test]
    fn findings_are_merged_across_operands_and_deduped() {
        // One finding per name, however many operands mentioned it.
        let evaluation = condition_holds(
            "'$(One)'=='$(Two)' Or '$(ONE)'=='x'",
            &Properties::new(),
            &site(),
            12,
        )
        .expect("evaluates");

        assert_eq!(evaluation.undefined, ["One", "Two"]);
    }

    #[test]
    fn a_deeply_nested_condition_is_refused() {
        // Recursive descent, so nesting is a stack-overflow vector on untrusted input — the same
        // class of bug MVP-35 owns for XML. Unlike that one, this test can actually run.
        let condition = format!("{}'a'=='a'{}", "(".repeat(100), ")".repeat(100));

        assert_eq!(refuse(&condition).kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn a_condition_at_the_nesting_limit_still_evaluates() {
        // The cap is a ceiling, not a limit below it.
        let depth = MAX_CONDITION_DEPTH;
        let condition = format!("{}'a'=='a'{}", "(".repeat(depth), ")".repeat(depth));

        assert!(holds_bare(&condition));
    }

    #[test]
    fn the_error_names_the_file_the_line_and_the_condition() {
        // A warning or failure about a condition is only actionable if the user can find it.
        let error = refuse("'1' < '2'");
        let message = error.to_string();

        assert!(
            message.contains("'1' < '2'"),
            "names the condition: {message}"
        );
        assert!(message.contains(":12:"), "names the line: {message}");
        assert!(message.contains("app.vcxproj"), "names the file: {message}");
    }

    // ---------------------------------------------------------------------------------------
    // MVP-12 — `%(Metadata)` expansion
    // ---------------------------------------------------------------------------------------

    /// A metadata bag holding `pairs`, the counterpart to `bag`.
    fn meta(pairs: &[(&str, &str)]) -> Metadata {
        let mut metadata = Metadata::new();
        for (name, value) in pairs {
            metadata.set(name, *value);
        }
        metadata
    }

    /// Expand against `metadata`, asserting nothing was recorded — what most cases want.
    fn substitute(input: &str, metadata: &Metadata) -> String {
        let expansion = expand_metadata(input, metadata).expect("expands");
        assert!(
            expansion.unsupported.is_empty(),
            "unexpected unsupported: {:?}",
            expansion.unsupported
        );
        expansion.value
    }

    #[test]
    fn an_unset_metadata_reference_expands_to_nothing_and_is_not_recorded() {
        // The entire inheritance mechanism, and the reason this is *silent* where `$(Undefined)` is
        // recorded. The first `ItemDefinitionGroup` in a graph writes
        // `WIN32;%(PreprocessorDefinitions)` against an empty bag, and every real project file does
        // this — recording it would put a warning on output that is completely correct, which is
        // exactly how a user learns to ignore the warning channel.
        let expansion =
            expand_metadata("WIN32;%(PreprocessorDefinitions)", &Metadata::new()).expect("expands");

        assert_eq!(expansion.value, "WIN32;");
        assert!(expansion.unsupported.is_empty());
        assert!(
            expansion.undefined.is_empty(),
            "an unset metadata name is not a finding: {:?}",
            expansion.undefined
        );
    }

    #[test]
    fn a_self_reference_composes_with_what_is_already_there() {
        // What the Visual Studio property page's "inherit from parent" checkbox produces. There is
        // no literal `%(Inherited)` token in MSBuild — this is what inheritance *is*.
        let metadata = meta(&[("PreprocessorDefinitions", "WIN32;NDEBUG")]);

        assert_eq!(
            substitute("%(PreprocessorDefinitions);EXTRA", &metadata),
            "WIN32;NDEBUG;EXTRA"
        );
        assert_eq!(
            substitute("EXTRA;%(PreprocessorDefinitions)", &metadata),
            "EXTRA;WIN32;NDEBUG",
            "position decides where the inherited list lands, which is why order is preserved"
        );
    }

    #[test]
    fn assignment_is_eager_so_a_self_reference_cannot_cycle() {
        // The same rule ADR 08 settled for properties: what is stored contains no sigil at all, so
        // `%(A)` resolves against the value from the *previous* assignment and appending twice
        // appends twice rather than looping.
        let mut metadata = meta(&[("Defines", "WIN32")]);
        let props = Properties::new();

        metadata
            .assign("Defines", "%(Defines);_DEBUG", &props)
            .map(drop)
            .expect("expands");
        assert_eq!(metadata.get("Defines"), Some("WIN32;_DEBUG"));

        metadata
            .assign("Defines", "%(Defines);UNICODE", &props)
            .map(drop)
            .expect("expands");
        assert_eq!(metadata.get("Defines"), Some("WIN32;_DEBUG;UNICODE"));
    }

    #[test]
    fn properties_expand_before_metadata() {
        // Order matters and only one order is right. A property's value was already expanded when
        // it was assigned, so it can hold no `%(`; but `$(CommonDefines)` legitimately holds one,
        // and doing `%` first would leave it as written.
        let props = bag(&[("CommonDefines", "%(PreprocessorDefinitions);SHARED")]);
        let mut metadata = meta(&[("PreprocessorDefinitions", "WIN32")]);

        metadata
            .assign("PreprocessorDefinitions", "$(CommonDefines)", &props)
            .map(drop)
            .expect("expands");

        assert_eq!(
            metadata.get("PreprocessorDefinitions"),
            Some("WIN32;SHARED")
        );
    }

    #[test]
    fn a_metadata_name_is_matched_case_insensitively() {
        // MSBuild matches metadata names the way it matches property names, and
        // `%(additionalincludedirectories)` reaching a different slot would surface as an include
        // path that quietly lost its inherited half.
        let metadata = meta(&[("AdditionalIncludeDirectories", "inc")]);

        assert_eq!(metadata.get("additionalincludedirectories"), Some("inc"));
        assert_eq!(
            substitute("%(ADDITIONALINCLUDEDIRECTORIES)", &metadata),
            "inc"
        );
    }

    #[test]
    fn an_empty_metadata_value_is_defined_and_distinct_from_absent() {
        // `<PreprocessorDefinitions></PreprocessorDefinitions>` clears the inherited list rather
        // than leaving it alone, so the two cases cannot be conflated — the argument
        // `Properties::get` makes, for the same reason.
        let metadata = meta(&[("Defined", "")]);

        assert_eq!(metadata.get("Defined"), Some(""));
        assert_eq!(metadata.get("Absent"), None);
    }

    #[test]
    fn a_qualified_reference_is_copied_through_and_recorded() {
        // `%(ClCompile.Identity)` and the rest of the well-known metadata are not modelled. Left as
        // written on purpose: a flag containing `%(ClCompile.Identity)` names its own cause, where
        // an empty one leaves a user with an include path that is simply missing.
        let expansion =
            expand_metadata("%(ClCompile.Identity)", &Metadata::new()).expect("expands");

        assert_eq!(expansion.value, "%(ClCompile.Identity)");
        assert_eq!(expansion.unsupported, ["%(ClCompile.Identity)"]);
    }

    #[test]
    fn an_item_transform_is_copied_through_and_recorded() {
        // `@(ClCompile->'%(Filename)')` is a real MSBuild construct and well outside this subset.
        let expansion = expand_metadata(
            "@(ClCompile);%(PreprocessorDefinitions)",
            &meta(&[("PreprocessorDefinitions", "WIN32")]),
        )
        .expect("expands");

        assert_eq!(
            expansion.value, "@(ClCompile);WIN32",
            "the transform survives verbatim and the supported reference beside it still expands"
        );
        assert_eq!(expansion.unsupported, ["@(ClCompile)"]);
    }

    #[test]
    fn an_unterminated_metadata_reference_is_copied_through_and_recorded() {
        // MSBuild fails the build on this. Recorded and copied through instead, matching what
        // `expand` does for `$(` — one stray sigil in metadata nothing reads should not fail a
        // generation that has a warning channel to report it on.
        let expansion = expand_metadata("inc;%(Unterminated", &Metadata::new()).expect("expands");

        assert_eq!(expansion.value, "inc;%(Unterminated");
        assert_eq!(expansion.unsupported, ["%(Unterminated"]);
    }

    #[test]
    fn findings_from_both_passes_reach_one_expansion() {
        // `Metadata::assign` runs two expansions and the caller reports once, so a value with a
        // problem of each kind must not lose either half on the way through.
        let mut metadata = Metadata::new();

        let expansion = metadata
            .assign(
                "AdditionalIncludeDirectories",
                "$(NoSuchProperty)inc;@(Headers)",
                &Properties::new(),
            )
            .expect("expands");

        assert_eq!(expansion.undefined, ["NoSuchProperty"]);
        assert_eq!(expansion.unsupported, ["@(Headers)"]);
    }

    #[test]
    fn a_runaway_metadata_self_reference_fails_at_the_size_cap() {
        // The `%` counterpart to `a_runaway_self_reference_fails_at_the_size_cap`. Eagerness rules
        // out a cycle but not doubling, and `assign` is again what has to stop it.
        let mut metadata = meta(&[("A", "x")]);
        let props = Properties::new();

        let error = (0..40)
            .try_for_each(|_| metadata.assign("A", "%(A)%(A)", &props).map(drop))
            .expect_err("must not be allowed to double indefinitely");

        assert_eq!(error.kind(), ErrorKind::RecursionLimit);
    }

    #[test]
    fn a_bare_percent_or_at_is_left_alone() {
        // `%` without a `(` is an MSBuild character escape (`%3B` is a literal semicolon) and `@`
        // appears in real paths. Neither is a reference, and treating either as one would corrupt
        // a value that is fine as it stands. The escapes are not *decoded* either: MVP-13 settled
        // that they stay as written, because a `%3B` reaching a flag is visible in the database
        // where a `%`-scanning warning would false-positive on every ordinary path. See ADR 13.
        let metadata = Metadata::new();

        assert_eq!(substitute("100%", &metadata), "100%");
        assert_eq!(substitute("a%3Bb", &metadata), "a%3Bb");
        assert_eq!(substitute("user@host", &metadata), "user@host");
    }

    // ---------------------------------------------------------------------------------------
    // MVP-13 — the wording the findings above are turned into
    // ---------------------------------------------------------------------------------------

    /// The two sentences existed in three copies before MVP-13 — `imports.rs`, and twice in
    /// `items.rs`. Pinning them here is what makes rewording one of them a single edit, and this
    /// module the place to look for the words.
    ///
    /// Both are warnings rather than notes. MVP-08 copies an unsupported construct through
    /// *verbatim* rather than emptying it, precisely so the bogus value names its own cause when it
    /// reaches a flag — and an undefined `$(X)` is how an include path quietly loses a directory.
    /// Neither is a curiosity; both mean the flags may be wrong. See ADR 13.
    #[test]
    fn the_two_finding_wordings_have_one_source() {
        let mut reporter = Reporter::new(false);

        report(
            &mut reporter,
            vec!["VCTargetsPath".to_string()],
            vec!["$(Foo.Substring(0))".to_string()],
            &site(),
            12,
        );

        let rendered: Vec<(Severity, &str)> = reporter
            .warnings()
            .iter()
            .map(|finding| (finding.severity, finding.message.as_str()))
            .collect();
        assert_eq!(
            rendered,
            [
                (
                    Severity::Warning,
                    "`$(VCTargetsPath)` is not defined and expanded to nothing"
                ),
                (
                    Severity::Warning,
                    "`$(Foo.Substring(0))` is not supported and was left as written"
                ),
            ]
        );
    }

    /// A finding is only actionable if it names where to go and look. `expand` and
    /// `condition_holds` do not know where their input was written, which is the whole reason the
    /// site is a parameter here rather than something the finding carries out of them.
    #[test]
    fn every_finding_is_sited_at_the_line_that_produced_it() {
        let mut reporter = Reporter::new(false);

        report(
            &mut reporter,
            vec!["Typo".to_string()],
            Vec::new(),
            &site(),
            40,
        );

        let finding = &reporter.warnings()[0];
        assert_eq!(finding.file.as_deref(), Some(site().as_path()));
        assert_eq!(finding.line, Some(40));
    }

    /// The consuming wrappers exist so a caller cannot report the same findings twice, and so
    /// reading the answer no longer needs a `.clone()` of the value before handing the struct on.
    #[test]
    fn reporting_an_expansion_hands_back_its_value() {
        let mut reporter = Reporter::new(false);

        let value = expand("$(Missing)tail", &Properties::new())
            .expect("expands")
            .report(&mut reporter, &site(), 7);

        assert_eq!(value, "tail");
        assert_eq!(reporter.warnings().len(), 1);
    }

    #[test]
    fn reporting_an_evaluation_hands_back_whether_it_holds() {
        let mut reporter = Reporter::new(false);

        let holds = condition_holds("'$(Missing)' == ''", &Properties::new(), &site(), 9)
            .expect("evaluates")
            .report(&mut reporter, &site(), 9);

        assert!(holds, "an undefined operand compares as empty");
        assert_eq!(reporter.warnings().len(), 1, "and is still reported");
    }
}
