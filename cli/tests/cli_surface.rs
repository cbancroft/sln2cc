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

#[test]
fn the_json_flag_is_positional_agnostic() {
    // The plugin builds argv programmatically, so accepting the flag in either position means
    // a reordering in `cli.lua` cannot silently produce undecodable output.
    let before = run(&["--json", "list-configs", "missing.sln"]);
    let after = run(&["list-configs", "missing.sln", "--json"]);

    assert_eq!(stdout(&before), stdout(&after));
    assert_eq!(code(&before), code(&after));
}
