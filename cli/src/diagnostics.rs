//! Non-fatal findings, and the `-v` progress channel they travel beside.
//!
//! A warning about an unsupported MSBuild construct is *data the plugin has to surface to the
//! user*, not a log line — the plugin is forbidden from scraping human-readable text, which is
//! why MVP-01 added no logging facade and why these end up in the JSON envelope's `warnings`
//! array instead. See [ADR 06](../../docs/adr/06-diagnostic-collection.md).
//!
//! [`Diagnostic`] and [`Severity`] live here rather than in `cli.rs` because by MVP-13 every
//! module in the evaluation pipeline produces diagnostics, and none of them should have to reach
//! into the CLI layer to do it. `cli.rs` still owns how they are *serialized*, as part of the
//! envelope.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A non-fatal problem found while evaluating a solution.
///
/// Warnings are the data the plugin surfaces to the user. Not log lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// The file the issue was found in, when it is attributable to one. Absolute, because the
    /// envelope is a contract with the plugin — see [`Diagnostic::render_human`] for why the
    /// terminal does not always see it that way.
    pub file: Option<PathBuf>,
    /// Line within `file`, when the parser knows it.
    pub line: Option<usize>,
}

impl Diagnostic {
    /// Write this diagnostic to a human-facing stream as one complete line.
    ///
    /// The `msvclsp:` prefix matches how `main.rs` renders errors, so warnings and failures line
    /// up in a terminal; it lives here rather than at the call site because the whole line is
    /// what the tests pin. `root` and `verbose` decide how much of the path is shown — see
    /// [`display_path`].
    pub fn render_human(&self, w: &mut dyn Write, root: &Path, verbose: bool) -> io::Result<()> {
        match self.site(root, verbose) {
            Some(site) => writeln!(w, "msvclsp: warning: {site}: {}", self.message),
            None => writeln!(w, "msvclsp: warning: {}", self.message),
        }
    }

    /// `path` or `path:line`, or `None` when the finding is not attributable to a file.
    fn site(&self, root: &Path, verbose: bool) -> Option<String> {
        let shown = display_path(self.file.as_deref()?, root, verbose);
        Some(match self.line {
            Some(line) => format!("{shown}:{line}"),
            None => shown,
        })
    }
}

/// How much a [`Diagnostic`] matters.
///
/// Only one level so far. Anything fatal is an [`Error`](crate::Error) instead. The variant's
/// wire string is written literally instead of derived from the identifier to avoid any issues
/// with renaming causing incompatibilities with the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    #[serde(rename = "warning")]
    Warning,
}

/// Collects diagnostics during a run, and carries the `-v` progress channel.
///
/// One object rather than two, for two reasons. Progress is inherently push-down: the point of
/// `-v` is feedback *while* a long parse is in flight, so it cannot be returned and printed at
/// the end. And warnings have to survive a failure — [`Envelope::with_warnings`] exists so a
/// failed run still reports what it found — whereas returning them up the call stack would mean
/// every `?` discarded everything collected below it. An accumulator the caller owns is simply
/// unaffected by propagation.
///
/// Keep this a standalone value rather than a field of whatever state the evaluator carries, or
/// `&mut Reporter` will collide with borrows of that state.
///
/// [`Envelope::with_warnings`]: crate::cli::Envelope::with_warnings
pub struct Reporter {
    warnings: Vec<Diagnostic>,
    /// `None` means quiet. Boxed so a test can read back what was written; the alternative, a
    /// `verbose: bool` and a bare `eprintln!`, leaves suppression with nothing to assert against.
    sink: Option<Box<dyn Write>>,
}

impl Reporter {
    /// A reporter whose progress goes to stderr when `verbose`, and nowhere otherwise.
    ///
    /// stderr, never stdout: under `--json` the plugin decodes stdout in its entirety.
    pub fn new(verbose: bool) -> Self {
        Reporter::with_sink(verbose.then(|| Box::new(io::stderr()) as Box<dyn Write>))
    }

    /// A reporter writing progress to an arbitrary sink, or discarding it when `None`.
    ///
    /// Exists for tests, and for any future caller that wants progress somewhere other than
    /// stderr — a log file, say — without `Reporter` having to know about it.
    pub fn with_sink(sink: Option<Box<dyn Write>>) -> Self {
        Reporter {
            warnings: Vec::new(),
            sink,
        }
    }

    /// Record a finding. Order is preserved, so warnings read back chronologically.
    pub fn warn(&mut self, diagnostic: Diagnostic) {
        self.warnings.push(diagnostic);
    }

    /// Record a warning against a file, which is what nearly every producer wants.
    pub fn warn_at(&mut self, file: &Path, line: Option<usize>, message: impl Into<String>) {
        self.warn(Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            file: Some(file.to_path_buf()),
            line,
        });
    }

    /// Report progress, if this reporter has anywhere to report it.
    ///
    /// Prefer the [`progress!`](crate::progress) macro at call sites. The write result is
    /// deliberately discarded: progress is courtesy output, and a closed stderr must not turn a
    /// successful generation into a failure.
    pub fn progress(&mut self, args: fmt::Arguments<'_>) {
        if let Some(sink) = self.sink.as_mut() {
            let _ = writeln!(sink, "msvclsp: {args}");
        }
    }

    /// The findings so far, in the order they were recorded.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// Take the findings, for attaching to the envelope once the run is over.
    pub fn into_warnings(self) -> Vec<Diagnostic> {
        self.warnings
    }
}

/// Report progress on a [`Reporter`], formatting nothing when the channel is off.
///
/// A macro rather than a method taking `&str` so the success path never builds a string it throws
/// away: `format_args!` only captures references, and [`Reporter::progress`] returns immediately
/// when there is no sink.
#[macro_export]
macro_rules! progress {
    ($reporter:expr, $($arg:tt)*) => {
        $reporter.progress(::std::format_args!($($arg)*))
    };
}

/// Shorten an absolute diagnostic path for display to a human.
///
/// Diagnostics carry absolute paths because the JSON envelope is a contract with the plugin. A
/// terminal is a different audience: CLAUDE.md §5 says not to put absolute user paths in anything
/// shareable, and stderr text is exactly what gets pasted into a bug report. So at default
/// verbosity the path is shown relative to the project root when it lives underneath it, and
/// `-v` is the explicit opt-in to the full path — which is what you want when the shortened form
/// is the thing confusing you.
fn display_path(file: &Path, root: &Path, verbose: bool) -> String {
    if verbose {
        return file.display().to_string();
    }

    // `root` comes from `--root` or the solution's own directory, either of which may be
    // relative. `strip_prefix` is purely lexical, so without absolutizing first it would fail to
    // match an absolute diagnostic path and every warning would print in full.
    let absolute = std::path::absolute(root);
    let root = absolute.as_deref().unwrap_or(root);

    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// A progress sink that stays readable after being boxed into a [`Reporter`].
    ///
    /// The `Rc<RefCell<_>>` is what lets the test keep a handle on the same buffer the reporter
    /// writes through. `borrow_mut` can panic on a double borrow, which is acceptable here in a
    /// way it would not be in a library path: nothing holds a borrow across the write.
    #[derive(Clone, Default)]
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).expect("progress output is UTF-8")
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A rooted project directory for the platform under test, matching `sln.rs`'s approach:
    /// `C:\proj` is a *relative* path on Linux CI, so `strip_prefix` would behave differently.
    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\proj")
        } else {
            PathBuf::from("/proj")
        }
    }

    fn under_root(parts: &[&str]) -> PathBuf {
        let mut path = root();
        parts.iter().for_each(|part| path.push(part));
        path
    }

    fn warning(file: Option<PathBuf>, line: Option<usize>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            message: "no `Format Version` header".to_string(),
            file,
            line,
        }
    }

    fn rendered(diagnostic: &Diagnostic, verbose: bool) -> String {
        let mut out = Vec::new();
        diagnostic
            .render_human(&mut out, &root(), verbose)
            .expect("writing a Vec cannot fail");
        String::from_utf8(out).expect("rendering is UTF-8")
    }

    #[test]
    fn diagnostics_read_back_in_the_order_they_were_recorded() {
        let mut reporter = Reporter::new(false);
        for message in ["first", "second", "third"] {
            reporter.warn_at(&under_root(&["app.sln"]), None, message);
        }

        let messages: Vec<&str> = reporter
            .warnings()
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert_eq!(messages, ["first", "second", "third"]);

        // `into_warnings` must not reorder either — it is what reaches the envelope.
        let taken: Vec<String> = reporter
            .into_warnings()
            .into_iter()
            .map(|warning| warning.message)
            .collect();
        assert_eq!(taken, ["first", "second", "third"]);
    }

    #[test]
    fn warn_at_records_the_file_and_line_it_is_given() {
        let mut reporter = Reporter::new(false);
        reporter.warn_at(&under_root(&["app.sln"]), Some(1), "unexpected version");

        assert_eq!(
            reporter.warnings(),
            [Diagnostic {
                severity: Severity::Warning,
                message: "unexpected version".to_string(),
                file: Some(under_root(&["app.sln"])),
                line: Some(1),
            }]
        );
    }

    /// The suppression this asserts is structural: quiet means there is no sink to write to, so
    /// no call site can accidentally bypass the check.
    #[test]
    fn verbosity_decides_whether_there_is_a_sink_at_all() {
        assert!(
            Reporter::new(false).sink.is_none(),
            "default verbosity must have nowhere to write progress"
        );
        assert!(Reporter::new(true).sink.is_some());
    }

    #[test]
    fn progress_reaches_an_enabled_sink() {
        let buffer = SharedBuf::default();
        let mut reporter = Reporter::with_sink(Some(Box::new(buffer.clone())));

        progress!(reporter, "reading {}", "app.sln");
        progress!(reporter, "{} projects", 3);

        assert_eq!(
            buffer.contents(),
            "msvclsp: reading app.sln\nmsvclsp: 3 projects\n"
        );
    }

    #[test]
    fn a_quiet_reporter_discards_progress_without_failing() {
        let mut reporter = Reporter::with_sink(None);
        progress!(reporter, "reading {}", "app.sln");
        assert!(reporter.warnings().is_empty(), "progress is not a warning");
    }

    #[test]
    fn a_path_under_the_root_is_shortened_at_default_verbosity() {
        let diagnostic = warning(Some(under_root(&["app", "app.vcxproj"])), Some(12));

        let expected = ["app", "app.vcxproj"].join(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            rendered(&diagnostic, false),
            format!("msvclsp: warning: {expected}:12: no `Format Version` header\n")
        );
    }

    #[test]
    fn verbose_shows_the_absolute_path() {
        let file = under_root(&["app", "app.vcxproj"]);
        let diagnostic = warning(Some(file.clone()), Some(12));

        assert_eq!(
            rendered(&diagnostic, true),
            format!(
                "msvclsp: warning: {}:12: no `Format Version` header\n",
                file.display()
            )
        );
    }

    /// `SolutionArgs::root()` returns `"."` for a bare `app.sln`, so the relative case is the
    /// common one, not an edge case. Without absolutizing the root first, the purely lexical
    /// `strip_prefix` would never match and every warning would print its full path.
    #[test]
    fn a_relative_root_is_resolved_before_stripping() {
        let cwd = std::env::current_dir().expect("the test process has a working directory");
        let diagnostic = warning(Some(cwd.join("app").join("app.vcxproj")), Some(7));

        let mut out = Vec::new();
        diagnostic
            .render_human(&mut out, Path::new("."), false)
            .expect("writing a Vec cannot fail");

        let expected = ["app", "app.vcxproj"].join(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            String::from_utf8(out).expect("rendering is UTF-8"),
            format!("msvclsp: warning: {expected}:7: no `Format Version` header\n")
        );
    }

    #[test]
    fn a_path_outside_the_root_stays_absolute() {
        // Projects living outside the solution tree are ordinary — ADR 04 resolves `..` rather
        // than refusing it — so there is nothing to shorten against.
        let file = if cfg!(windows) {
            PathBuf::from(r"C:\shared\lib\lib.vcxproj")
        } else {
            PathBuf::from("/shared/lib/lib.vcxproj")
        };
        let diagnostic = warning(Some(file.clone()), None);

        assert_eq!(
            rendered(&diagnostic, false),
            format!(
                "msvclsp: warning: {}: no `Format Version` header\n",
                file.display()
            )
        );
    }

    #[test]
    fn a_diagnostic_with_no_file_renders_the_message_alone() {
        assert_eq!(
            rendered(&warning(None, None), false),
            "msvclsp: warning: no `Format Version` header\n"
        );
    }

    #[test]
    fn severity_serializes_as_its_wire_string() {
        assert_eq!(
            serde_json::to_value(warning(None, Some(4))).expect("serializes"),
            serde_json::json!({
                "severity": "warning",
                "message": "no `Format Version` header",
                "file": null,
                "line": 4,
            })
        );
    }
}
