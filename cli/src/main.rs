//! Thin entry point. Argument parsing and dispatch only — see `lib.rs` for the real work.
//!
//! This file owns exactly three things the library deliberately does not: which stream each
//! kind of output goes to, whether it is rendered as JSON or as text, and the process exit
//! code. Everything else lives behind [`cli::run`].

use std::error::Error as StdError;
use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

use msvclsp::cli::{self, Cli, Envelope, Outcome, Render};
use msvclsp::{Diagnostic, Error, Reporter};

fn main() -> ExitCode {
    // A usage error never reaches us: clap writes its own message to stderr and exits 2,
    // which is why no ErrorKind is allowed to claim that code.
    let cli = Cli::parse();

    // Progress goes to the reporter's own channel as it happens; warnings accumulate and are
    // attached below, to whichever envelope the run produces.
    let mut reporter = Reporter::new(cli.verbose);
    let outcome = cli::run(&cli, &mut reporter);
    let warnings = reporter.into_warnings();

    // Before either branch: a warning explains a result, so it has to precede it, and it must
    // not be the last thing on stderr when there is also an error to read.
    write_warnings(&cli, &warnings);

    match outcome {
        Ok(outcome) => match write_success(&cli, &outcome, warnings) {
            Ok(()) => ExitCode::SUCCESS,
            // A failed write is usually a closed pipe. Report it, but do not pretend the
            // command itself failed in a way the plugin should classify.
            Err(error) => {
                let _ = writeln!(io::stderr(), "msvclsp: writing output failed: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            write_failure(&cli, &error, warnings);
            exit_code(&error)
        }
    }
}

/// Write the successful result to stdout, as either the envelope or human-readable text.
///
/// Never both and never a mix: under `--json` the plugin decodes stdout in its entirety.
fn write_success(cli: &Cli, outcome: &Outcome, warnings: Vec<Diagnostic>) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    if cli.json {
        writeln!(
            out,
            "{}",
            encode(&Envelope::ok(outcome).with_warnings(warnings))
        )?;
    } else {
        outcome.render_human(&mut out)?;
    }

    out.flush()
}

/// Report a failure: the human-readable form on stderr always, the envelope on stdout under
/// `--json`.
///
/// stderr carries the message in *both* modes because MVP-28 folds its contents into the
/// plugin's own structured error whenever the exit code is non-zero.
fn write_failure(cli: &Cli, error: &Error, warnings: Vec<Diagnostic>) {
    // Write errors are ignored throughout this function: the process is already failing, and
    // the exit code is the part that still has to be right.
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let _ = writeln!(err, "msvclsp: {error}");

    // Error::Context renders only its own message, so the chain is where the specifics are.
    let mut cause = error.source();
    while let Some(current) = cause {
        let _ = writeln!(err, "  caused by: {current}");
        cause = current.source();
    }

    if cli.json {
        // The turbofish is unavoidable on this path: there is no `Outcome` value to infer the
        // envelope's type parameter from.
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let envelope = Envelope::<&Outcome>::err(error).with_warnings(warnings);
        let _ = writeln!(out, "{}", encode(&envelope));
        let _ = out.flush();
    }
}

/// Report accumulated warnings on stderr, in *both* output modes.
///
/// Under `--json` these also ride in the envelope, which is what the plugin reads; the duplicate
/// on stderr is deliberate. stderr is never parsed, and a captured run should still be legible to
/// a human without decoding JSON by hand.
fn write_warnings(cli: &Cli, warnings: &[Diagnostic]) {
    if warnings.is_empty() {
        return;
    }

    // The root is where paths are shortened against at default verbosity, so it comes from the
    // same place `.msvclsp/` would: `--root`, or the solution's own directory.
    let root = cli.command.solution_args().root();

    let stderr = io::stderr();
    let mut err = stderr.lock();
    for warning in warnings {
        // Ignored for the same reason as in `write_failure`: a warning that cannot be printed
        // must not change the outcome of the run.
        let _ = warning.render_human(&mut err, &root, cli.verbose);
    }
}

/// Serialize an envelope to compact JSON.
///
/// The `expect` is sound rather than merely convenient: `serde_json` fails only on a custom
/// `Serialize` impl returning an error, a map with non-string keys, or a non-finite float.
/// Everything reachable from an envelope is a derived struct over strings, paths, integers,
/// and bools, so none of those are constructible here.
fn encode<T: serde::Serialize>(envelope: &T) -> String {
    serde_json::to_string(envelope).expect("envelope types cannot fail to serialize")
}

/// Map a failure onto its process exit code.
fn exit_code(error: &Error) -> ExitCode {
    // Every value in ErrorKind::exit_code's table fits in a u8. The fallback keeps that from
    // being an invariant this function has to assert.
    ExitCode::from(u8::try_from(error.kind().exit_code()).unwrap_or(1))
}
