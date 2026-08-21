//! Subcommand definitions and the JSON output types that form the contract with the
//! Neovim plugin.
//!
//! Anything the plugin consumes programmatically must be emitted as structured JSON,
//! never human-formatted text.

use std::{
    error::Error as StdError,
    io,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::toolchain::ToolchainProvider;
use crate::{
    ConfigPlatform, Diagnostic, Error, Reporter, compdb, db,
    msbuild::{self, ProjectScope},
    progress, sln,
};

/// Version of the envelope shape below. Bumped when a change would break a plugin
/// that decoded the previous version. No change needed if we are just adding a field.
const SCHEMA_VERSION: u32 = 1;

/// Result of the `list-configs` subcommand.
#[derive(Debug, Clone, Serialize)]
pub struct SolutionConfigs {
    pub solution: PathBuf,
    pub available: Vec<ConfigPlatform>,
    /// Last-used selection for this solution, if one has been recorded.
    pub active: Option<ConfigPlatform>,
    /// Resolved output path for the `compile_commands.json` for this solution, whether it has been
    /// generated or not.
    pub database: PathBuf,
}

/// Result of the `generate` subcommand.
///
/// MVP-22 will widen this eventually.
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
/// flags when deciding how to render whatever comes back. `reporter` is borrowed rather than
/// returned because warnings have to survive a failure — see [`Reporter`].
pub fn run(
    cli: &Cli,
    reporter: &mut Reporter,
    toolchain: &dyn ToolchainProvider,
) -> crate::Result<Outcome> {
    let args = cli.command.solution_args();
    progress!(
        reporter,
        "{}: {}",
        cli.command.name(),
        args.solution.display()
    );

    match &cli.command {
        Command::ListConfigs { solution } => {
            list_configs(solution, reporter).map(Outcome::ListConfigs)
        }
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
            generate(solution, &requested, *force, toolchain, reporter).map(Outcome::Generate)
        }
    }
}

fn list_configs(args: &SolutionArgs, reporter: &mut Reporter) -> crate::Result<SolutionConfigs> {
    let solution = sln::parse_sln(&args.solution, reporter)?;

    // `db::Store::open` wants `.sln2cc` itself, not a per-solution hashed subdirectory.
    let sln2cc_dir = args.root().join(".sln2cc");
    let store = db::Store::open(&sln2cc_dir)?;
    let active = store.active_config(&solution.path)?;
    let output_dir = compdb::resolve_output_dir(&sln2cc_dir, &solution.path)?;

    Ok(SolutionConfigs {
        solution: solution.path,
        available: solution.configurations,
        active,
        database: output_dir.join("compile_commands.json"),
    })
}

fn generate(
    args: &SolutionArgs,
    requested: &ConfigPlatform,
    force: bool,
    toolchain: &dyn ToolchainProvider,
    reporter: &mut Reporter,
) -> crate::Result<GenerateResult> {
    let solution = sln::parse_sln(&args.solution, reporter)?;

    if !solution.configurations.contains(requested) {
        return Err(Error::InvalidConfiguration {
            requested: requested.clone(),
            available: solution.configurations.clone(),
        });
    }

    let sln2cc_dir = args.root().join(".sln2cc");
    let mut store = db::Store::open(&sln2cc_dir)?;
    let output_dir = compdb::resolve_output_dir(&sln2cc_dir, &solution.path)?;
    let database = output_dir.join("compile_commands.json");

    let regenerated = if !force && store.is_up_to_date(&solution.path)? {
        false
    } else {
        let discovered = toolchain.discover()?;
        let mut evaluated = Vec::new();
        let mut dependencies = vec![solution.path.clone()];

        for project in &solution.projects {
            let Some(mapping) = project.config_for(requested) else {
                continue;
            };

            let scope = ProjectScope {
                solution: solution.path.clone(),
                project: project.path.clone(),
                config: mapping.project.clone(),
                toolchain: Some(discovered.clone()),
            };
            let result = msbuild::evaluate(&scope, reporter)?;
            dependencies.extend(result.dependencies.iter().cloned());
            evaluated.push(result);
        }

        let commands = compdb::build(&evaluated, &discovered)?;
        compdb::write(&commands, &output_dir)?;
        store.record_dependencies(&solution.path, &dependencies)?;
        true
    };

    store.set_active_config(&solution.path, requested)?;

    Ok(GenerateResult {
        database,
        regenerated,
    })
}

/// Main CLI commands for `sln2cc`
#[derive(Debug, PartialEq, Parser)]
#[command(
    name = "sln2cc",
    version,
    about = "Generate a clangd compilation database from a Visual Studio Solution",
    long_about = None
    )]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Emit a machine-readable JSON structure on stdout instead of
    /// human-readable text.
    //
    // The Neovim plugin will always use this. When present, nothing but the
    // envelope is written to stdout so that it can be decoded directly.
    //
    // `display_order` keeps the global flags below each subcommand's own arguments in
    // `--help`; without it they interleave, splitting `--configuration` from `--platform`.
    #[arg(long, global = true, display_order = 100)]
    pub json: bool,

    /// Report progress on stderr while parsing.
    ///
    /// Parsing a large solution may take time, this can be used to keep track
    /// of what is happening. Stdout is unaffected.
    #[arg(short, long, global = true, display_order = 101)]
    pub verbose: bool,
}

/// The subcommands for the plugin.
///
/// There is no `discover`. Locating a solution is the Neovim plugin's job
/// (`nvim/lua/sln2cc/discovery.lua`) — see [ADR 44](../../docs/adr/44-drop-cli-side-discovery.md).
/// The CLI is always handed a solution path, never asked to find one.
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
    pub fn solution_args(&self) -> &SolutionArgs {
        match self {
            Command::ListConfigs { solution } | Command::Generate { solution, .. } => solution,
        }
    }

    /// The subcommand's name as spelled on the command line.
    ///
    /// Written out rather than derived, for the same reason [`ErrorKind::as_str`] is: these
    /// strings are what the plugin passes in `argv`, so a rename must not silently change them.
    ///
    /// [`ErrorKind::as_str`]: crate::ErrorKind::as_str
    pub fn name(&self) -> &'static str {
        match self {
            Command::ListConfigs { .. } => "list-configs",
            Command::Generate { .. } => "generate",
        }
    }
}

/// Arguments shared by every subcommand: solution and root
#[derive(Debug, PartialEq, Args)]
pub struct SolutionArgs {
    /// Path to the `.sln` file to read.
    pub solution: PathBuf,

    /// Project root to write the `.sln2cc/` under. Defaults to solution directory.
    ///
    /// The plugin passes the root it detected from Neovim's LSP root markers, which
    /// is often not the directory holding the solution.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

impl SolutionArgs {
    /// The directory `.sln2cc/` is created under.
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
/// Field order is declaration order in the emitted JSON, to keep things diffable.
#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub schema_version: u32,
    pub has_error: bool,
    pub result: Option<T>,
    pub error: Option<ErrorPayload>,

    /// Non-fatal problems found while producing `result`. Always present, but empty when there
    /// were no issues found. Accumulated by a [`Reporter`] and attached by `main.rs`, so a run
    /// that failed still reports what it found on the way — see [`Envelope::with_warnings`].
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

    /// A deterministic toolchain for tests that need `run`'s `&dyn ToolchainProvider` argument but
    /// never actually reach evaluation — mirrors `compdb.rs`'s own local `toolchain()` helper.
    fn fake_toolchain() -> crate::toolchain::FakeToolchain {
        crate::toolchain::FakeToolchain(crate::toolchain::Toolchain {
            install_path: PathBuf::from(if cfg!(windows) { r"D:\vs" } else { "/vs" }),
            vc_targets_path: PathBuf::from(if cfg!(windows) {
                r"D:\vs\MSBuild\Microsoft\VC\v170"
            } else {
                "/vs/MSBuild/Microsoft/VC/v170"
            }),
            include_paths: Vec::new(),
            toolset_version: "14.38.33130".to_string(),
            sdk_version: "10.0.22621.0".to_string(),
        })
    }

    /// clap's own consistency check over the derived definition. Catches duplicate flags,
    /// conflicting short options, and malformed help before any of them reach a user.
    #[test]
    fn the_command_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_list_configs() {
        let cli = Cli::try_parse_from(["sln2cc", "list-configs", "app.sln"]).expect("parses");

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
            "sln2cc",
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
        let before = Cli::try_parse_from(["sln2cc", "--json", "-v", "list-configs", "app.sln"])
            .expect("parses");
        let after = Cli::try_parse_from(["sln2cc", "list-configs", "app.sln", "--json", "-v"])
            .expect("parses");

        assert_eq!(before, after);
        assert!(before.json && before.verbose);
    }

    /// The plugin passes `--json` on every call, so both subcommands have to accept it.
    /// MVP-28 should extend this to assert against the argv `cli.lua` actually builds, once
    /// it builds any.
    #[test]
    fn both_subcommands_accept_json() {
        assert!(Cli::try_parse_from(["sln2cc", "list-configs", "a.sln", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "sln2cc",
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

    /// MVP-28's `cli.lua` builds this exact argv for every call: solution, `--root`, and
    /// `--json`, plus `--configuration`/`--platform` for `generate`. Pinning it here means a
    /// rename on either side of the plugin/binary boundary breaks a test instead of shipping
    /// a plugin that silently stops parsing.
    #[test]
    fn matches_the_argv_cli_lua_builds() {
        let cli = Cli::try_parse_from([
            "sln2cc",
            "list-configs",
            "app.sln",
            "--root",
            "C:/proj",
            "--json",
        ])
        .expect("parses");
        assert_eq!(
            cli.command,
            Command::ListConfigs {
                solution: solution_args("app.sln", Some("C:/proj")),
            }
        );
        assert!(cli.json);

        let cli = Cli::try_parse_from([
            "sln2cc",
            "generate",
            "app.sln",
            "--configuration",
            "Debug",
            "--platform",
            "x64",
            "--root",
            "C:/proj",
            "--json",
        ])
        .expect("parses");
        assert_eq!(
            cli.command,
            Command::Generate {
                solution: solution_args("app.sln", Some("C:/proj")),
                configuration: "Debug".to_string(),
                platform: "x64".to_string(),
                force: false,
            }
        );
        assert!(cli.json);
    }

    #[test]
    fn missing_required_arguments_are_usage_errors() {
        let cases: [&[&str]; 3] = [
            &["sln2cc", "list-configs"],
            &["sln2cc", "generate", "a.sln", "--platform", "x64"],
            &["sln2cc", "generate", "a.sln", "--configuration", "Debug"],
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
        // Built rather than written as a `C:\`-prefixed literal: that literal is a single opaque
        // component on the Linux CI runner, so `Path::parent()` never splits it the way it does
        // on Windows.
        let mut proj = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        proj.push("proj");
        let mut solution = proj.clone();
        solution.push("app.sln");
        let mut workspace = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        workspace.push("workspace");

        assert_eq!(
            solution_args(&solution.to_string_lossy(), None).root(),
            proj
        );
        assert_eq!(
            solution_args(
                &solution.to_string_lossy(),
                Some(&workspace.to_string_lossy())
            )
            .root(),
            workspace
        );
        // `Path::parent` yields an empty path for a bare filename, not `None`.
        assert_eq!(solution_args("app.sln", None).root(), PathBuf::from("."));
    }

    /// Proves `generate` actually reaches `sln::parse_sln` rather than short-circuiting on
    /// `Error::Unimplemented` — MVP-22 retires that variant entirely (per ADR 03, "whichever of
    /// MVP-21/MVP-22 lands second"), so a nonexistent solution must surface as `SolutionNotFound`.
    /// `run` takes the toolchain provider as its third argument as of this issue — see
    /// [ADR 24](../../docs/adr/24-generate-wiring.md) — and this reaches `FakeToolchain` never
    /// getting called at all, since parsing fails before evaluation would need it.
    #[test]
    fn generate_propagates_a_missing_solution() {
        let cli = Cli::try_parse_from([
            "sln2cc",
            "generate",
            "no-such-solution.sln",
            "--configuration",
            "Debug",
            "--platform",
            "x64",
        ])
        .expect("parses");
        let mut reporter = Reporter::new(false);
        let toolchain = fake_toolchain();
        let error = run(&cli, &mut reporter, &toolchain).expect_err("the solution does not exist");

        assert_eq!(error.kind(), crate::ErrorKind::SolutionNotFound);
    }

    /// Proves `list_configs` actually reaches `sln::parse_sln` rather than short-circuiting: a
    /// nonexistent solution must surface as `SolutionNotFound`, not the retired `Unimplemented`.
    #[test]
    fn list_configs_propagates_a_missing_solution() {
        let cli = Cli::try_parse_from(["sln2cc", "list-configs", "no-such-solution.sln"])
            .expect("parses");
        let mut reporter = Reporter::new(false);
        let toolchain = fake_toolchain();
        let error = run(&cli, &mut reporter, &toolchain).expect_err("the solution does not exist");

        assert_eq!(error.kind(), crate::ErrorKind::SolutionNotFound);
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

    /// The failure case is the invariant worth pinning: a run that collected warnings and *then*
    /// failed still has to report them, which is why `with_warnings` is a builder separate from
    /// `ok` and `err` rather than a constructor argument.
    #[test]
    fn warnings_ride_on_both_envelope_shapes() {
        let warning = || Diagnostic {
            severity: crate::Severity::Warning,
            message: "no `Format Version` header".to_string(),
            file: Some(PathBuf::from(r"C:\proj\app.sln")),
            line: None,
        };
        let expected = json!([{
            "severity": "warning",
            "message": "no `Format Version` header",
            "file": r"C:\proj\app.sln",
            "line": null,
        }]);

        let ok = serde_json::to_value(Envelope::ok("payload").with_warnings(vec![warning()]))
            .expect("serializes");
        assert_eq!(ok["has_error"], json!(false));
        assert_eq!(ok["warnings"], expected);

        let err = serde_json::to_value(
            Envelope::<&str>::err(&Error::SolutionNotFound {
                path: PathBuf::from(r"C:\proj\missing.sln"),
            })
            .with_warnings(vec![warning()]),
        )
        .expect("serializes");
        assert_eq!(err["has_error"], json!(true));
        assert_eq!(err["result"], json!(null));
        assert_eq!(err["warnings"], expected);
    }

    /// These strings are what the plugin puts in `argv`, and `run` reports one as progress.
    #[test]
    fn subcommand_names_match_the_command_line() {
        let name = |args: &[&str]| Cli::try_parse_from(args).expect("parses").command.name();

        assert_eq!(name(&["sln2cc", "list-configs", "a.sln"]), "list-configs");
        assert_eq!(
            name(&[
                "sln2cc",
                "generate",
                "a.sln",
                "--configuration",
                "Debug",
                "--platform",
                "x64",
            ]),
            "generate"
        );
    }

    #[test]
    fn result_serializes_as_a_bare_object() {
        let outcome = Outcome::Generate(GenerateResult {
            database: PathBuf::from(r"C:\proj\.sln2cc\app\compile_commands.json"),
            regenerated: true,
        });

        assert_eq!(
            serde_json::to_value(Envelope::ok(outcome)).expect("serializes"),
            json!({
                "schema_version": 1,
                "has_error": false,
                "result": {
                    "database": r"C:\proj\.sln2cc\app\compile_commands.json",
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
            database: PathBuf::from(r"C:\proj\...\compile_commands.json"),
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
