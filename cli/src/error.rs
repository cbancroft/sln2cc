//! The crate's error type and its machine-readable classification.
//!
//! Errors here are not just messages. The Neovim plugin has to tell "solution not found"
//! apart from "unsupported MSBuild construct" apart from "no Visual Studio installation" in
//! order to map a failure onto an exit code and a JSON error kind, which is why `anyhow`'s
//! type erasure was rejected in favour of a real enum — see
//! [ADR 02](../../docs/adr/02-structured-error-type.md).
//!
//! Two types carry that classification, deliberately kept apart:
//!
//! - [`Error`] is the Rust-facing value, carrying the offending path, line, or construct so
//!   the human-readable message can be specific.
//! - [`ErrorKind`] is the wire-facing label. Its strings are a contract with the plugin, so
//!   they are written out explicitly rather than derived from variant names — renaming a
//!   variant must not silently change what the plugin sees.
//!
//! Adding context to an error must never destroy its kind; see [`Error::context`].

use std::path::{Path, PathBuf};

use crate::{ConfigPlatform, Result};

/// Every way an operation in this crate can fail.
///
/// One flat enum rather than per-module error types: the crate is small enough that nested
/// enums would cost conversion boilerplate and a `kind()` delegated through three layers,
/// without buying isolation that matters here.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed. The path is carried separately because
    /// `std::io::Error` does not include it, and it is the most useful part of the message.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("solution not found: {}", path.display())]
    SolutionNotFound { path: PathBuf },

    #[error("{}:{line}: malformed solution file: {detail}", path.display())]
    SolutionParse {
        path: PathBuf,
        line: usize,
        detail: String,
    },

    #[error("{}: malformed project XML: {source}", path.display())]
    ProjectParse {
        path: PathBuf,
        #[source]
        source: roxmltree::Error,
    },

    /// An MSBuild construct outside the supported subset. Reported rather than skipped,
    /// because a silently ignored `PropertyGroup` yields a plausible but wrong database.
    #[error("{}: unsupported MSBuild construct: {construct}", format_site(file, *line))]
    UnsupportedConstruct {
        file: PathBuf,
        line: Option<usize>,
        construct: String,
    },

    #[error(
        "{}: imported file not found: {}",
        referenced_by.display(),
        import.display()
    )]
    MissingImport {
        import: PathBuf,
        referenced_by: PathBuf,
    },

    #[error("import cycle: {}", join_paths(chain, " -> "))]
    ImportCycle { chain: Vec<PathBuf> },

    /// A depth cap was hit — a cyclic `$(Macro)` reference or an import chain too deep to be
    /// plausible. Failing here is what keeps hostile project files from hanging the tool.
    #[error("recursion limit of {limit} exceeded while {context}")]
    RecursionLimit { limit: usize, context: String },

    /// No usable Visual Studio installation. Covers both "`vswhere` is absent" and "no
    /// instance carries the C++ workload": the remediation is the same either way, so the
    /// specifics go in `detail` rather than splitting a kind the plugin cannot act on.
    #[error("no usable Visual Studio installation found: {detail}")]
    ToolchainNotFound { detail: String },

    /// Visual Studio is installed but the toolset or Windows SDK layout is not as expected.
    /// Distinct from [`Error::ToolchainNotFound`] because the fix is different — repair or
    /// install an SDK, rather than install Visual Studio.
    #[error("{}: incomplete toolchain: {detail}", install_root.display())]
    ToolchainIncomplete {
        install_root: PathBuf,
        detail: String,
    },

    #[error(
        "{requested} is not a configuration of this solution; valid values are: {}",
        join_configs(available)
    )]
    InvalidConfiguration {
        requested: ConfigPlatform,
        available: Vec<ConfigPlatform>,
    },

    #[error("{}: malformed state file: {source}", path.display())]
    StateParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "{}: state file version {found} is not supported (expected {supported})",
        path.display()
    )]
    StateVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// A path derived from a project file resolved outside the tree it is allowed to reach.
    /// Solution and project files are untrusted input; `..` traversal is rejected, not
    /// followed.
    #[error("{}: path escapes {}", path.display(), root.display())]
    PathEscape { path: PathBuf, root: PathBuf },

    /// Wraps another error with context describing what was being attempted. Its
    /// [`kind`](Error::kind) is the wrapped error's — see [`Error::context`].
    #[error("{context}")]
    Context {
        context: String,
        #[source]
        source: Box<Error>,
    },

    /// Used for unimplemented methods as development begins. Will be dropped 
    /// during MVP-21/MVP-22.
    #[error("{feature} is not implemented yet")]
    Unimplemented {
        feature: String,
    },
}

impl Error {
    /// The wire-facing classification of this error.
    ///
    /// [`Error::Context`] delegates to what it wraps, so adding context can never change how
    /// the plugin classifies a failure.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Io { .. } => ErrorKind::Io,
            Error::SolutionNotFound { .. } => ErrorKind::SolutionNotFound,
            Error::SolutionParse { .. } => ErrorKind::SolutionParse,
            Error::ProjectParse { .. } => ErrorKind::ProjectParse,
            Error::UnsupportedConstruct { .. } => ErrorKind::UnsupportedConstruct,
            Error::MissingImport { .. } => ErrorKind::MissingImport,
            Error::ImportCycle { .. } => ErrorKind::ImportCycle,
            Error::RecursionLimit { .. } => ErrorKind::RecursionLimit,
            Error::ToolchainNotFound { .. } => ErrorKind::ToolchainNotFound,
            Error::ToolchainIncomplete { .. } => ErrorKind::ToolchainIncomplete,
            Error::InvalidConfiguration { .. } => ErrorKind::InvalidConfiguration,
            Error::StateParse { .. } => ErrorKind::StateParse,
            Error::StateVersion { .. } => ErrorKind::StateVersion,
            Error::PathEscape { .. } => ErrorKind::PathEscape,
            Error::Context { source, .. } => source.kind(),
            Error::Unimplemented { .. } => ErrorKind::Unimplemented
        }
    }

    /// Construct an [`Error::Io`], attaching the path `std::io::Error` omits.
    ///
    /// There is deliberately no `From<std::io::Error>` impl: a bare `?` on a file read would
    /// discard the path, which is the part worth having.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    /// Wrap this error with a description of what was being attempted, preserving its
    /// [`kind`](Error::kind).
    ///
    /// MSBuild evaluation is deeply nested — "evaluating `a.props`, imported by
    /// `b.vcxproj`" is exactly the context that makes a failure actionable — so wrapping has
    /// to be cheap *and* has to leave the machine-readable classification intact.
    pub fn context(self, context: impl Into<String>) -> Self {
        Error::Context {
            context: context.into(),
            source: Box::new(self),
        }
    }
}

/// Adds [`Error::context`] to a fallible call without unwrapping it first.
///
/// The message is built lazily so the success path never allocates.
pub trait ResultExt<T> {
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> ResultExt<T> for Result<T> {
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| e.context(context()))
    }
}

/// The machine-readable classification the plugin dispatches on.
///
/// The string values in [`ErrorKind::as_str`] are a published contract, which is why this
/// type exists at all rather than the plugin reading Rust variant names: a variant can be
/// renamed or split without the plugin noticing, and a wire string cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Io,
    SolutionNotFound,
    SolutionParse,
    ProjectParse,
    UnsupportedConstruct,
    MissingImport,
    ImportCycle,
    RecursionLimit,
    ToolchainNotFound,
    ToolchainIncomplete,
    InvalidConfiguration,
    StateParse,
    StateVersion,
    PathEscape,
    Unimplemented,
}

impl ErrorKind {
    /// Every kind, in declaration order. Exists so tests can assert the wire strings stay
    /// unique and unchanged without listing them twice.
    pub const ALL: [ErrorKind; 15] = [
        ErrorKind::Io,
        ErrorKind::SolutionNotFound,
        ErrorKind::SolutionParse,
        ErrorKind::ProjectParse,
        ErrorKind::UnsupportedConstruct,
        ErrorKind::MissingImport,
        ErrorKind::ImportCycle,
        ErrorKind::RecursionLimit,
        ErrorKind::ToolchainNotFound,
        ErrorKind::ToolchainIncomplete,
        ErrorKind::InvalidConfiguration,
        ErrorKind::StateParse,
        ErrorKind::StateVersion,
        ErrorKind::PathEscape,
        ErrorKind::Unimplemented,
    ];

    /// The stable identifier emitted in JSON. Changing one of these is a breaking change to
    /// the plugin contract.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Io => "io",
            ErrorKind::SolutionNotFound => "solution-not-found",
            ErrorKind::SolutionParse => "solution-parse",
            ErrorKind::ProjectParse => "project-parse",
            ErrorKind::UnsupportedConstruct => "unsupported-construct",
            ErrorKind::MissingImport => "missing-import",
            ErrorKind::ImportCycle => "import-cycle",
            ErrorKind::RecursionLimit => "recursion-limit",
            ErrorKind::ToolchainNotFound => "toolchain-not-found",
            ErrorKind::ToolchainIncomplete => "toolchain-incomplete",
            ErrorKind::InvalidConfiguration => "invalid-configuration",
            ErrorKind::StateParse => "state-parse",
            ErrorKind::StateVersion => "state-version",
            ErrorKind::PathEscape => "path-escape",
            ErrorKind::Unimplemented => "unimplemented",
        }
    }

    /// Process exit code for this kind.
    ///
    /// Coarse classes rather than one code per kind: the plugin reads the precise kind from
    /// JSON, so exit codes only need to be useful from a shell. `2` is never returned — it
    /// is what `clap` uses for usage errors, and overloading it would make "you typed the
    /// command wrong" indistinguishable from a real failure.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Io => 1,
            ErrorKind::SolutionNotFound => 3,
            ErrorKind::SolutionParse
            | ErrorKind::ProjectParse
            | ErrorKind::StateParse
            | ErrorKind::StateVersion => 4,
            ErrorKind::UnsupportedConstruct
            | ErrorKind::MissingImport
            | ErrorKind::ImportCycle
            | ErrorKind::RecursionLimit => 5,
            ErrorKind::ToolchainNotFound | ErrorKind::ToolchainIncomplete => 6,
            ErrorKind::InvalidConfiguration => 7,
            ErrorKind::PathEscape => 8,
            ErrorKind::Unimplemented => 9,
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Renders `path` or `path:line` for messages where the line number is optional.
fn format_site(file: &Path, line: Option<usize>) -> String {
    match line {
        Some(line) => format!("{}:{line}", file.display()),
        None => file.display().to_string(),
    }
}

fn join_paths(paths: &[PathBuf], separator: &str) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(separator)
}

fn join_configs(configs: &[ConfigPlatform]) -> String {
    configs
        .iter()
        .map(ConfigPlatform::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::error::Error as _;
    use std::io;

    use super::*;

    fn config(configuration: &str, platform: &str) -> ConfigPlatform {
        ConfigPlatform {
            configuration: configuration.to_string(),
            platform: platform.to_string(),
        }
    }

    /// One sample per variant, so `Display` and `kind()` can be checked exhaustively without
    /// each test rebuilding the list.
    fn one_of_each() -> Vec<(Error, ErrorKind)> {
        vec![
            (
                Error::io(
                    r"C:\proj\app.sln",
                    io::Error::from(io::ErrorKind::PermissionDenied),
                ),
                ErrorKind::Io,
            ),
            (
                Error::SolutionNotFound {
                    path: PathBuf::from(r"C:\proj\missing.sln"),
                },
                ErrorKind::SolutionNotFound,
            ),
            (
                Error::SolutionParse {
                    path: PathBuf::from(r"C:\proj\app.sln"),
                    line: 12,
                    detail: "truncated Project entry".to_string(),
                },
                ErrorKind::SolutionParse,
            ),
            (
                Error::ProjectParse {
                    path: PathBuf::from(r"C:\proj\app.vcxproj"),
                    source: roxmltree::Document::parse("<a>").unwrap_err(),
                },
                ErrorKind::ProjectParse,
            ),
            (
                Error::UnsupportedConstruct {
                    file: PathBuf::from(r"C:\proj\app.vcxproj"),
                    line: Some(40),
                    construct: "HasTrailingSlash('$(OutDir)')".to_string(),
                },
                ErrorKind::UnsupportedConstruct,
            ),
            (
                Error::MissingImport {
                    import: PathBuf::from(r"C:\proj\shared.props"),
                    referenced_by: PathBuf::from(r"C:\proj\app.vcxproj"),
                },
                ErrorKind::MissingImport,
            ),
            (
                Error::ImportCycle {
                    chain: vec![
                        PathBuf::from(r"C:\proj\a.props"),
                        PathBuf::from(r"C:\proj\b.props"),
                        PathBuf::from(r"C:\proj\a.props"),
                    ],
                },
                ErrorKind::ImportCycle,
            ),
            (
                Error::RecursionLimit {
                    limit: 64,
                    context: "expanding $(Outer)".to_string(),
                },
                ErrorKind::RecursionLimit,
            ),
            (
                Error::ToolchainNotFound {
                    detail: "vswhere.exe not present".to_string(),
                },
                ErrorKind::ToolchainNotFound,
            ),
            (
                Error::ToolchainIncomplete {
                    install_root: PathBuf::from(r"C:\VS\2022\Community"),
                    detail: "no Windows SDK include directory".to_string(),
                },
                ErrorKind::ToolchainIncomplete,
            ),
            (
                Error::InvalidConfiguration {
                    requested: config("Profile", "ARM64"),
                    available: vec![config("Debug", "x64"), config("Release", "x64")],
                },
                ErrorKind::InvalidConfiguration,
            ),
            (
                Error::StateParse {
                    path: PathBuf::from(r"C:\proj\.msvclsp\state.json"),
                    source: serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
                },
                ErrorKind::StateParse,
            ),
            (
                Error::StateVersion {
                    path: PathBuf::from(r"C:\proj\.msvclsp\state.json"),
                    found: 9,
                    supported: 1,
                },
                ErrorKind::StateVersion,
            ),
            (
                Error::PathEscape {
                    path: PathBuf::from(r"C:\Windows\System32"),
                    root: PathBuf::from(r"C:\proj"),
                },
                ErrorKind::PathEscape,
            ),
            (
                Error::Unimplemented { feature: "test_function".to_string() },
                ErrorKind::Unimplemented,
            ),
        ]
    }

    #[test]
    fn every_variant_has_a_kind() {
        let covered: HashSet<ErrorKind> = one_of_each().iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            covered.len(),
            ErrorKind::ALL.len(),
            "one_of_each() must cover every ErrorKind"
        );

        for (error, expected) in one_of_each() {
            assert_eq!(error.kind(), expected, "wrong kind for {error:?}");
        }
    }

    #[test]
    fn display_names_the_offending_input() {
        let rendered: Vec<String> = one_of_each()
            .iter()
            .map(|(error, _)| error.to_string())
            .collect();

        assert_eq!(rendered[0], r"C:\proj\app.sln: permission denied");
        assert_eq!(rendered[1], r"solution not found: C:\proj\missing.sln");
        assert_eq!(
            rendered[2],
            r"C:\proj\app.sln:12: malformed solution file: truncated Project entry"
        );
        assert!(rendered[3].starts_with(r"C:\proj\app.vcxproj: malformed project XML: "));
        assert_eq!(
            rendered[4],
            r"C:\proj\app.vcxproj:40: unsupported MSBuild construct: HasTrailingSlash('$(OutDir)')"
        );
        assert_eq!(
            rendered[5],
            r"C:\proj\app.vcxproj: imported file not found: C:\proj\shared.props"
        );
        assert_eq!(
            rendered[6],
            r"import cycle: C:\proj\a.props -> C:\proj\b.props -> C:\proj\a.props"
        );
        assert_eq!(
            rendered[7],
            "recursion limit of 64 exceeded while expanding $(Outer)"
        );
        assert_eq!(
            rendered[8],
            "no usable Visual Studio installation found: vswhere.exe not present"
        );
        assert_eq!(
            rendered[9],
            r"C:\VS\2022\Community: incomplete toolchain: no Windows SDK include directory"
        );
        assert_eq!(
            rendered[10],
            "Profile|ARM64 is not a configuration of this solution; \
             valid values are: Debug|x64, Release|x64"
        );
        assert!(rendered[11].starts_with(r"C:\proj\.msvclsp\state.json: malformed state file: "));
        assert_eq!(
            rendered[12],
            r"C:\proj\.msvclsp\state.json: state file version 9 is not supported (expected 1)"
        );
        assert_eq!(rendered[13], r"C:\Windows\System32: path escapes C:\proj");
    }

    #[test]
    fn unsupported_construct_renders_without_a_line() {
        let error = Error::UnsupportedConstruct {
            file: PathBuf::from(r"C:\proj\app.vcxproj"),
            line: None,
            construct: "item transform".to_string(),
        };
        assert_eq!(
            error.to_string(),
            r"C:\proj\app.vcxproj: unsupported MSBuild construct: item transform"
        );
    }

    /// The wire strings are a contract with the plugin, so they are asserted literally here:
    /// changing one has to be a deliberate edit to this list, not a rename side effect.
    #[test]
    fn kind_strings_are_stable() {
        let expected = [
            "io",
            "solution-not-found",
            "solution-parse",
            "project-parse",
            "unsupported-construct",
            "missing-import",
            "import-cycle",
            "recursion-limit",
            "toolchain-not-found",
            "toolchain-incomplete",
            "invalid-configuration",
            "state-parse",
            "state-version",
            "path-escape",
            "unimplemented"
        ];
        let actual: Vec<&str> = ErrorKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn kind_strings_are_unique() {
        let unique: HashSet<&str> = ErrorKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(unique.len(), ErrorKind::ALL.len());
    }

    #[test]
    fn kind_displays_as_its_wire_string() {
        assert_eq!(
            ErrorKind::SolutionNotFound.to_string(),
            "solution-not-found"
        );
    }

    #[test]
    fn exit_codes_are_grouped_and_never_collide_with_clap() {
        assert_eq!(ErrorKind::Io.exit_code(), 1);
        assert_eq!(ErrorKind::SolutionNotFound.exit_code(), 3);
        assert_eq!(ErrorKind::SolutionParse.exit_code(), 4);
        assert_eq!(ErrorKind::StateVersion.exit_code(), 4);
        assert_eq!(ErrorKind::UnsupportedConstruct.exit_code(), 5);
        assert_eq!(ErrorKind::ImportCycle.exit_code(), 5);
        assert_eq!(ErrorKind::ToolchainIncomplete.exit_code(), 6);
        assert_eq!(ErrorKind::InvalidConfiguration.exit_code(), 7);
        assert_eq!(ErrorKind::PathEscape.exit_code(), 8);

        for kind in ErrorKind::ALL {
            let code = kind.exit_code();
            assert_ne!(code, 0, "{kind} must not report success");
            assert_ne!(
                code, 2,
                "{kind} must not collide with clap's usage-error code"
            );
        }
    }

    #[test]
    fn context_preserves_the_kind() {
        for (error, expected) in one_of_each() {
            let wrapped = error.context("reading the solution").context("generating");
            assert_eq!(wrapped.kind(), expected);
        }
    }

    #[test]
    fn context_replaces_the_message_and_keeps_the_cause() {
        let inner = Error::SolutionNotFound {
            path: PathBuf::from(r"C:\proj\missing.sln"),
        };
        let wrapped = inner.context("listing configurations");

        assert_eq!(wrapped.to_string(), "listing configurations");
        let cause = wrapped.source().expect("context must expose its cause");
        assert_eq!(
            cause.to_string(),
            r"solution not found: C:\proj\missing.sln"
        );
    }

    #[test]
    fn source_chain_reaches_the_innermost_cause() {
        let error = Error::io(
            r"C:\proj\a.props",
            io::Error::from(io::ErrorKind::PermissionDenied),
        )
        .context("evaluating a.props")
        .context("evaluating app.vcxproj");

        let mut chain: Vec<String> = vec![error.to_string()];
        let mut current: Option<&dyn std::error::Error> = error.source();
        while let Some(err) = current {
            chain.push(err.to_string());
            current = err.source();
        }

        assert_eq!(
            chain,
            vec![
                "evaluating app.vcxproj".to_string(),
                "evaluating a.props".to_string(),
                r"C:\proj\a.props: permission denied".to_string(),
                "permission denied".to_string(),
            ]
        );
    }

    #[test]
    fn kind_survives_propagation_through_a_call_chain() {
        fn innermost() -> Result<()> {
            Err(Error::ToolchainNotFound {
                detail: "no instance with the C++ workload".to_string(),
            })
        }

        fn middle() -> Result<()> {
            innermost().with_context(|| "discovering the toolchain".to_string())?;
            Ok(())
        }

        fn outermost() -> Result<()> {
            middle().with_context(|| "generating compile_commands.json".to_string())?;
            Ok(())
        }

        let error = outermost().expect_err("the innermost call always fails");
        assert_eq!(error.kind(), ErrorKind::ToolchainNotFound);
        assert_eq!(error.kind().exit_code(), 6);
        assert_eq!(error.to_string(), "generating compile_commands.json");
    }

    #[test]
    fn with_context_leaves_success_untouched() {
        let ok: Result<u32> = Ok(7);
        let value = ok
            .with_context(|| unreachable!("the message must not be built on success"))
            .expect("Ok must pass through");
        assert_eq!(value, 7);
    }
}
