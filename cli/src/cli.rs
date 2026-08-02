//! Subcommand definitions and the JSON output types that form the contract with the
//! Neovim plugin.
//!
//! Anything the plugin consumes programmatically must be emitted as structured JSON,
//! never human-formatted text.

use std::io;
use std::{
    error::Error as StdError,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{ConfigPlatform, Error};

/// Version of the envelope shape below. Bumped when a change would break a plugin
/// that decoded the previous version. No change needed if we are just adding a field.
const SCHEMA_VERSION: u32 = 1;

/// Result of the `discover` subcommand.
#[derive(Debug, Clone)]
pub struct DiscoveredSolution {
    pub path: PathBuf,
    /// Name used to namespace artifacts under `.msvclsp/<name>/`.
    pub name: String,
}

/// Result of the `list-configs` subcommand.
#[derive(Debug, Clone, Serialize)]
pub struct SolutionConfigs {
    pub solution: PathBuf,
    pub available: Vec<ConfigPlatform>,
    /// Last-used selection for this solution, if one has been recorded.
    pub active: Option<ConfigPlatform>,
}

/// Result of the `generate` subcommand.
///
/// MVP-22 will widen this eventually
#[derive(Debug, Clone, Serialize)]
pub struct GenerateResult {
    /// Absolute path of the written `compile_commands.json`.
    pub database: PathBuf,
    /// `true` if the compile_commands.json needed to be rewritten. Used
    /// to indicate if clangd needs to be restarted
    pub regenerated: bool,
}

/// Encapsulates the result of a subcommand executing so that `main.rs`
/// has only one type to render.
///
/// `untagged` makes the envelope's `result` the base result instead of being
/// tagged with the type of result.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Outcome {
    ListConfigs(SolutionConfigs),
    Generate(GenerateResult),
}

/// Human-readable string render, used when `--json` is not specified.
///
/// Separate from `Display` because these write multiple lines to a stream rather
/// than producing a value. Also keeping it a distinct trait makes it obvious
/// which types are user-facing vs internal.
pub trait Render {
    fn render_human(&self, w: &mut dyn Write) -> io::Result<()>;
}

impl Render for Outcome {
    fn render_human(&self, w: &mut dyn Write) -> io::Result<()> {
        match self {
            Outcome::ListConfigs(result) => result.render_human(w),
            Outcome::Generate(result) => result.render_human(w),
        }
    }
}

impl Render for SolutionConfigs {
    fn render_human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "Configurations for {}:", self.solution.display())?;

        if self.available.is_empty() {
            return writeln!(w, "  (none declared)");
        }

        for config in &self.available {
            let marker = if Some(config) == self.active.as_ref() {
                "  (active)"
            } else {
                ""
            };
            writeln!(w, "  {config}{marker}")?;
        }

        Ok(())
    }
}

impl Render for GenerateResult {
    fn render_human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.regenerated {
            writeln!(w, "Wrote {}", self.database.display())
        } else {
            writeln!(w, "Already up to date: {}", self.database.display())
        }
    }
}

/// Dispatch a parsed command line to its handler.
///
/// Takes `&Cli` rather than consuming it so `main.rs` can still read the `--json` and `--verbose`
/// flags when deciding how to render whatever comes back.
pub fn run(cli: &Cli) -> crate::Result<Outcome> {
    match &cli.command {
        Command::ListConfigs { solution } => list_configs(solution).map(Outcome::ListConfigs),
        Command::Generate {
            solution,
            configuration,
            platform,
            force,
        } => {
            let requested = ConfigPlatform {
                configuration: configuration.clone(),
                platform: platform.clone(),
            };
            generate(solution, &requested, *force).map(Outcome::Generate)
        }
    }
}

/// MVP-21 implements this.
fn list_configs(_args: &SolutionArgs) -> crate::Result<SolutionConfigs> {
    Err(Error::Unimplemented {
        feature: "list-configs".to_string(),
    })
}

/// MVP-22 implements this
fn generate(
    _args: &SolutionArgs,
    _requested: &ConfigPlatform,
    _force: bool,
) -> crate::Result<GenerateResult> {
    Err(Error::Unimplemented {
        feature: "generate".to_string(),
    })
}

/// Main CLI commands for `msvclsp`
///
#[derive(Debug, PartialEq, Parser)]
#[command(
    name = "msvclsp",
    version,
    about = "Generate a clangd compilation database from a Visual Studio Solution",
    long_about = None
    )]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Emit a machine-readable JSON structure on stdout instead of
    /// human-readable text.
    ///
    /// The Neovim plugin will always use this. When present, nothing but the
    /// envelope is written to stdout so that it can be decoded directly.
    ///
    /// `display_order` keeps the global flags below each subcommand's own arguments in
    /// `--help`; without it they interleave, splitting `--configuration` from `--platform`.
    #[arg(long, global = true, display_order = 100)]
    pub json: bool,

    /// Report progress on stderr while parsing.
    ///
    /// Parsing a large solution may take time, this can be used to keep track
    /// of what is happening. Stdout is unaffected.
    #[arg(short, long, global = true, display_order = 101)]
    pub verbose: bool,
}

///The subcommands for the plugin.
///
/// `discover` is absent for now. Solution scanning will be implemented here
/// in V1-01. For now, scanning is done in the Neovim plugin.
#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    /// List the configuration/platform pairs a solution declares.
    #[command(name = "list-configs")]
    ListConfigs {
        #[command(flatten)]
        solution: SolutionArgs,
    },

    /// Generate `compile_commands.json` for one configuration/platform.
    #[command(name = "generate")]
    Generate {
        #[command(flatten)]
        solution: SolutionArgs,

        /// Configuration to evaluate, as the solution spells it -- e.g. `Debug`.
        #[arg(long, short)]
        configuration: String,

        /// Platform to evaluate, as the solution spells it -- e.g. `x64`.
        #[arg(long, short)]
        platform: String,

        /// Regenerate even when no input file has changed since the last run.
        #[arg(long)]
        force: bool,
    },
}

impl Command {
    /// The solution arguments, whichever subcommand was invoked.
    ///
    /// Lets `main.rs` and the diagnostics channel reach the solution path without matching on
    /// every variant.
    pub fn solution_args(&self) -> &SolutionArgs {
        match self {
            Command::ListConfigs { solution } | Command::Generate { solution, .. } => solution,
        }
    }
}

/// Arguments shared by every subcommand: solution and root
#[derive(Debug, PartialEq, Args)]
pub struct SolutionArgs {
    /// Path to the `.sln` file to read.
    pub solution: PathBuf,

    /// Project root to write the `.msvclsp/` under. Defaults to solution directory.
    ///
    /// The plugin passes the root it detected from the Neovim's LSP root markers, which
    /// is often not the directory holding the solution.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

impl SolutionArgs {
    /// The directory `.msvclsp/` is created under.
    ///
    /// Falling back to the solutions directory keeps things
    /// functional even if a root isn't specified. `Path::parent` yields
    /// an empty path for a bare filename like `app.sln` which is why the result is
    /// filtered rather than used directly.
    pub fn root(&self) -> PathBuf {
        if let Some(root) = &self.root {
            return root.clone();
        }

        self.solution
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }
}

/// The object written out as part of the `--json` invocation.
/// Field order is declaration order in the omitted JSON, to keep things diffable.
#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub schema_version: u32,
    pub has_error: bool,
    pub result: Option<T>,
    pub error: Option<ErrorPayload>,

    /// Non-fatal problemes found while producing `result`. Always present, but
    /// empty when there were no issues found. Accumulation should be done in MVP-04.
    pub warnings: Vec<Diagnostic>,
}

impl<T> Envelope<T> {
    /// A successful response with no warnings.
    pub fn ok(result: T) -> Self {
        Envelope {
            schema_version: SCHEMA_VERSION,
            has_error: false,
            result: Some(result),
            error: None,
            warnings: Vec::new(),
        }
    }

    /// A failed response. Takes the error by reference because the caller still may need to
    /// use it for other message output.
    pub fn err(error: &Error) -> Self {
        Envelope {
            schema_version: SCHEMA_VERSION,
            has_error: true,
            result: None,
            error: Some(ErrorPayload::new(error)),
            warnings: Vec::new(),
        }
    }

    /// Attach warnings collected during the run.
    ///
    /// Separate from the constructors because warnings accumulate
    /// independently of whether the run actually succeeded.
    pub fn with_warnings(mut self, warnings: Vec<Diagnostic>) -> Self {
        self.warnings = warnings;
        self
    }
}

/// The machine-readable half of an error.
///
/// `kind` is [`ErrorKind::as_str`](crate::ErrorKind::as_str) rather than the enum itself.
/// `ErrorKind` deliberately derives no serde traits. The decision to put its wire string
/// on the wire is made here as it owns the payload shape.
#[derive(Debug, Serialize)]
pub struct ErrorPayload {
    pub kind: &'static str,
    /// The outermost error's `Display`, which for a wrapped error is the context description
    /// rather than the underlying cause.
    pub message: String,

    /// The `source()` chain, outermost-inward. Gives the plugin something to show beyond the
    /// one-line summary without having to parse it back apart.
    pub causes: Vec<String>,
}

impl ErrorPayload {
    fn new(error: &Error) -> Self {
        let mut causes = Vec::new();
        let mut current = error.source();
        while let Some(cause) = current {
            causes.push(cause.to_string());
            current = cause.source();
        }

        ErrorPayload {
            kind: error.kind().as_str(),
            message: error.to_string(),
            causes,
        }
    }
}

/// A non-fatal problem found while evaluating a solution.
///
/// Warnings are the data the plugin surfaces to the user. Not log lines.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// The file the issue was found in, when it is attributable to one.
    pub file: Option<PathBuf>,
    /// Line within `file`, when the parser knows it.
    pub line: Option<usize>,
}

/// How much a [`Diagnostic`] matters.
///
/// Only one level so far. Anything fatal is an [`Error`] instead. The variants wire
/// string is written literally instead of derived from the identifier to avoid any issues
/// with renaming causing incompatibilities with the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    #[serde(rename = "warning")]
    Warning,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::error::ErrorKind as ClapErrorKind;
    use serde_json::json;

    use super::*;

    fn solution_args(solution: &str, root: Option<&str>) -> SolutionArgs {
        SolutionArgs {
            solution: PathBuf::from(solution),
            root: root.map(PathBuf::from),
        }
    }

    /// clap's own consistency check over the derived definition. Catches duplicate flags,
    /// conflicting short options, and malformed help before any of them reach a user.
    #[test]
    fn the_command_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_list_configs() {
        let cli = Cli::try_parse_from(["msvclsp", "list-configs", "app.sln"]).expect("parses");

        assert_eq!(
            cli.command,
            Command::ListConfigs {
                solution: solution_args("app.sln", None),
            }
        );
        assert!(!cli.json);
        assert!(!cli.verbose);
    }

    #[test]
    fn parses_generate_with_every_flag() {
        let cli = Cli::try_parse_from([
            "msvclsp",
            "generate",
            "app.sln",
            "--configuration",
            "Debug",
            "--platform",
            "x64",
            "--root",
            "..",
            "--force",
        ])
        .expect("parses");

        assert_eq!(
            cli.command,
            Command::Generate {
                solution: solution_args("app.sln", Some("..")),
                configuration: "Debug".to_string(),
                platform: "x64".to_string(),
                force: true,
            }
        );
    }

    #[test]
    fn global_flags_parse_in_either_position() {
        let before = Cli::try_parse_from(["msvclsp", "--json", "-v", "list-configs", "app.sln"])
            .expect("parses");
        let after = Cli::try_parse_from(["msvclsp", "list-configs", "app.sln", "--json", "-v"])
            .expect("parses");

        assert_eq!(before, after);
        assert!(before.json && before.verbose);
    }

    /// The plugin passes `--json` on every call, so both subcommands have to accept it.
    /// MVP-28 should extend this to assert against the argv `cli.lua` actually builds, once
    /// it builds any.
    #[test]
    fn both_subcommands_accept_json() {
        assert!(Cli::try_parse_from(["msvclsp", "list-configs", "a.sln", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "msvclsp",
                "generate",
                "a.sln",
                "--configuration",
                "Debug",
                "--platform",
                "x64",
                "--json",
            ])
            .is_ok()
        );
    }

    #[test]
    fn missing_required_arguments_are_usage_errors() {
        let cases: [&[&str]; 3] = [
            &["msvclsp", "list-configs"],
            &["msvclsp", "generate", "a.sln", "--platform", "x64"],
            &["msvclsp", "generate", "a.sln", "--configuration", "Debug"],
        ];

        for args in cases {
            let error = Cli::try_parse_from(args).expect_err("must not parse");
            assert_eq!(
                error.kind(),
                ClapErrorKind::MissingRequiredArgument,
                "wrong error for {args:?}"
            );
        }
    }

    #[test]
    fn root_defaults_to_the_solutions_directory() {
        assert_eq!(
            solution_args(r"C:\proj\app.sln", None).root(),
            PathBuf::from(r"C:\proj")
        );
        assert_eq!(
            solution_args(r"C:\proj\app.sln", Some(r"C:\workspace")).root(),
            PathBuf::from(r"C:\workspace")
        );
        // `Path::parent` yields an empty path for a bare filename, not `None`.
        assert_eq!(solution_args("app.sln", None).root(), PathBuf::from("."));
    }

    #[test]
    fn handlers_report_themselves_unimplemented() {
        let cases: [(&[&str], &str); 2] = [
            (&["msvclsp", "list-configs", "a.sln"], "list-configs"),
            (
                &[
                    "msvclsp",
                    "generate",
                    "a.sln",
                    "--configuration",
                    "Debug",
                    "--platform",
                    "x64",
                ],
                "generate",
            ),
        ];

        for (args, expected) in cases {
            let cli = Cli::try_parse_from(args).expect("parses");
            let error = run(&cli).expect_err("no handler is implemented yet");

            assert_eq!(error.kind(), crate::ErrorKind::Unimplemented);
            assert_eq!(
                error.to_string(),
                format!("{expected} is not implemented yet")
            );
            assert_eq!(error.kind().exit_code(), 9);
        }
    }

    #[test]
    fn success_and_failure_share_one_key_set() {
        let ok = serde_json::to_value(Envelope::ok("payload")).expect("serializes");
        let err = serde_json::to_value(Envelope::<&str>::err(&Error::SolutionNotFound {
            path: PathBuf::from(r"C:\proj\missing.sln"),
        }))
        .expect("serializes");

        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("envelope is an object")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&ok), keys(&err));

        assert_eq!(
            ok,
            json!({
                "schema_version": 1,
                "has_error": false,
                "result": "payload",
                "error": null,
                "warnings": [],
            })
        );

        assert_eq!(
            err,
            json!({
                "schema_version": 1,
                "has_error": true,
                "result": null,
                "error": {
                    "kind": "solution-not-found",
                    "message": r"solution not found: C:\proj\missing.sln",
                    "causes": [],
                },
                "warnings": [],
            })
        );
    }

    #[test]
    fn result_serializes_as_a_bare_object() {
        let outcome = Outcome::Generate(GenerateResult {
            database: PathBuf::from(r"C:\proj\.msvclsp\app\compile_commands.json"),
            regenerated: true,
        });

        assert_eq!(
            serde_json::to_value(Envelope::ok(outcome)).expect("serializes"),
            json!({
                "schema_version": 1,
                "has_error": false,
                "result": {
                    "database": r"C:\proj\.msvclsp\app\compile_commands.json",
                    "regenerated": true,
                },
                "error": null,
                "warnings": [],
            })
        );
    }

    #[test]
    fn human_rendering_marks_the_active_pair() {
        let config = |conf: &str, plat: &str| ConfigPlatform {
            configuration: conf.to_string(),
            platform: plat.to_string(),
        };

        let configs = SolutionConfigs {
            solution: PathBuf::from(r"C:\proj\app.sln"),
            available: vec![config("Debug", "x64"), config("Release", "x64")],
            active: Some(config("Release", "x64")),
        };

        let mut rendered = Vec::new();
        configs
            .render_human(&mut rendered)
            .expect("writing a Vec cannot fail");

        assert_eq!(
            String::from_utf8(rendered).expect("rendering is UTF-8"),
            "Configurations for C:\\proj\\app.sln:\n  Debug|x64\n  Release|x64  (active)\n"
        );
    }
}
