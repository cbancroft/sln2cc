//! Integration tests over the built binary's argument surface.
//!
//! These assert what a unit test cannot reach: real process exit codes, and which stream each
//! kind of output lands on. The stream discipline is the contract the plugin depends on —
//! under `--json`, stdout must be decodable in its entirety — so it is asserted here rather
//! than assumed.

use std::process::{Command, Output};

/// Cargo builds the binary before running integration tests and exposes its path here, so
/// these tests need neither `assert_cmd` nor a hardcoded target directory.
const BIN: &str = env!("CARGO_BIN_EXE_msvclsp");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("cargo builds the binary before running integration tests")
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("output is UTF-8")
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("output is UTF-8")
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the binary exits normally rather than being signalled")
}

/// Assert that stdout under `--json` is decodable in its entirety: exactly one line, and that
/// line a single JSON object.
///
/// Checked structurally rather than with a JSON parser, matching the exact-string style of the
/// assertions below. This is the invariant the plugin depends on, and it is worth an assertion of
/// its own because the way it breaks is a stray `print!` somewhere deep in evaluation — which is
/// invisible to every test that only looks at what it expected to find.
fn assert_stdout_is_one_json_object(output: &Output, context: &str) {
    let out = stdout(output);
    let mut lines = out.lines();

    let Some(line) = lines.next() else {
        panic!("stdout was empty for {context}");
    };
    assert!(
        line.starts_with('{') && line.ends_with('}'),
        "stdout for {context} is not a bare JSON object:\n{out}"
    );
    assert_eq!(
        lines.next(),
        None,
        "stdout for {context} carried more than the envelope:\n{out}"
    );
}

#[test]
fn help_names_both_subcommands() {
    let output = run(&["--help"]);
    assert_eq!(code(&output), 0);

    let help = stdout(&output);
    assert!(help.contains("list-configs"), "help was:\n{help}");
    assert!(help.contains("generate"), "help was:\n{help}");
    // `discover` is V1-01. If it shows up here, the surface drifted ahead of the plan.
    assert!(!help.contains("discover"), "help was:\n{help}");
}

#[test]
fn subcommand_help_succeeds() {
    for subcommand in ["list-configs", "generate"] {
        let output = run(&[subcommand, "--help"]);
        assert_eq!(code(&output), 0, "{subcommand} --help failed");
        assert!(
            stdout(&output).contains("--json"),
            "{subcommand} --help hid the flag the plugin depends on"
        );
    }
}

#[test]
fn version_succeeds() {
    let output = run(&["--version"]);
    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn usage_errors_exit_two_and_leave_stdout_empty() {
    // stdout staying empty is the invariant worth protecting: the plugin decodes it, and
    // clap's usage text is not decodable. Exit 2 is reserved for exactly this case, which is
    // why no ErrorKind returns it.
    let cases: [&[&str]; 4] = [
        &["list-configs"],                                    // missing <SOLUTION>
        &["generate", "app.sln", "--configuration", "Debug"], // missing --platform
        &["not-a-subcommand"],
        &[], // no subcommand at all
    ];

    for args in cases {
        let output = run(args);
        assert_eq!(code(&output), 2, "wrong exit code for {args:?}");
        assert_eq!(stdout(&output), "", "stdout was not empty for {args:?}");
        assert!(!stderr(&output).is_empty(), "stderr was empty for {args:?}");
    }
}

#[test]
fn unimplemented_subcommands_emit_an_envelope_under_json() {
    let output = run(&["list-configs", "missing.sln", "--json"]);

    assert_eq!(code(&output), 9);
    // Asserted as an exact string rather than a parsed value: this pins the key set, the key
    // order, and the wire strings in one place, and needs no JSON parser in the test crate.
    assert_eq!(
        stdout(&output),
        concat!(
            r#"{"schema_version":1,"has_error":true,"result":null,"#,
            r#""error":{"kind":"unimplemented","#,
            r#""message":"list-configs is not implemented yet","causes":[]},"#,
            r#""warnings":[]}"#,
            "\n"
        )
    );
    // The human message is on stderr in both modes — MVP-28 folds it into the plugin's error.
    assert!(stderr(&output).contains("not implemented yet"));
}

#[test]
fn unimplemented_subcommands_leave_stdout_empty_without_json() {
    let output = run(&[
        "generate",
        "missing.sln",
        "--configuration",
        "Debug",
        "--platform",
        "x64",
    ]);

    assert_eq!(code(&output), 9);
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("generate is not implemented yet"));
}

/// Every `--json` invocation the tool accepts, checked against the one-object rule in one place.
///
/// The list is the reachable surface today; MVP-21 and MVP-22 should extend it to the paths that
/// actually parse a solution, which is where a stray print is most likely to appear.
#[test]
fn json_stdout_is_always_exactly_one_object() {
    let cases: [&[&str]; 4] = [
        &["list-configs", "missing.sln", "--json"],
        &["list-configs", "missing.sln", "--json", "-v"],
        &[
            "generate",
            "missing.sln",
            "--configuration",
            "Debug",
            "--platform",
            "x64",
            "--json",
        ],
        &[
            "generate",
            "missing.sln",
            "--configuration",
            "Debug",
            "--platform",
            "x64",
            "--json",
            "-v",
        ],
    ];

    for args in cases {
        assert_stdout_is_one_json_object(&run(args), &format!("{args:?}"));
    }
}

#[test]
fn verbose_reports_progress_on_stderr_without_touching_stdout() {
    let quiet = run(&["list-configs", "missing.sln", "--json"]);
    let loud = run(&["list-configs", "missing.sln", "--json", "-v"]);

    // Progress must never change what the plugin decodes — it is the same run either way.
    assert_eq!(stdout(&quiet), stdout(&loud));
    assert_eq!(code(&quiet), code(&loud));

    // The progress line names the subcommand and the solution it was handed.
    let progress = "msvclsp: list-configs: missing.sln";
    assert!(
        stderr(&loud).contains(progress),
        "stderr under -v was:\n{}",
        stderr(&loud)
    );
    assert!(
        !stderr(&quiet).contains(progress),
        "default verbosity must not report progress; stderr was:\n{}",
        stderr(&quiet)
    );
}

#[test]
fn verbose_does_not_write_progress_to_stdout_in_human_mode() {
    // Without `--json` stdout is human-readable text, but progress still belongs on stderr:
    // anything a caller pipes must be the result, not the narration.
    let output = run(&["list-configs", "missing.sln", "-v"]);

    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("msvclsp: list-configs: missing.sln"));
}

/// MVP-21 unblocks this: `list-configs` currently returns `Error::Unimplemented` without ever
/// reaching `sln::parse_sln`, so no CLI path produces a warning yet. The plumbing either side of
/// that gap is unit-tested in `diagnostics.rs` and `cli.rs`; this is the end-to-end assertion,
/// left visible rather than absent.
#[test]
#[ignore = "no subcommand reaches sln::parse_sln until MVP-21"]
fn warnings_ride_in_the_envelope() {
    todo!(
        "run list-configs against a .sln with no `Format Version` header; assert the envelope's \
         warnings array carries the diagnostic and that the same message appears on stderr"
    )
}

#[test]
fn the_json_flag_is_positional_agnostic() {
    // The plugin builds argv programmatically, so accepting the flag in either position means
    // a reordering in `cli.lua` cannot silently produce undecodable output.
    let before = run(&["--json", "list-configs", "missing.sln"]);
    let after = run(&["list-configs", "missing.sln", "--json"]);

    assert_eq!(stdout(&before), stdout(&after));
    assert_eq!(code(&before), code(&after));
}
