//! Integration tests over the built binary's argument surface.
//!
//! These assert what a unit test cannot reach: real process exit codes, and which stream each
//! kind of output lands on. The stream discipline is the contract the plugin depends on —
//! under `--json`, stdout must be decodable in its entirety — so it is asserted here rather
//! than assumed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Cargo builds the binary before running integration tests and exposes its path here, so
/// these tests need neither `assert_cmd` nor a hardcoded target directory.
const BIN: &str = env!("CARGO_BIN_EXE_msvclsp");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("cargo builds the binary before running integration tests")
}

/// A directory removed when the test that made it ends, holding a `.sln` (and whatever else a
/// test writes into it, e.g. a seeded `.msvclsp/state.json`).
///
/// `cli/src/testing.rs::TempTree` is `pub(crate)` inside the lib crate and unreachable from this
/// integration-test binary, so this is a small reimplementation rather than a shared dependency.
/// `fixtures/simple` and `fixtures/multi-config` are not an alternative: they are still
/// README-only stubs (MVP-23/MVP-24 own authoring them), and list-configs needs nothing beyond a
/// `.sln` text file to exercise.
struct TempSolution(PathBuf);

impl TempSolution {
    /// A fresh directory named for the calling test, containing `app.sln` with `body` as its
    /// contents. The process id and a counter keep concurrent test binaries and parallel tests
    /// within one from colliding.
    fn new(name: &str, body: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let dir = std::env::temp_dir().join(format!(
            "msvclsp-cli-surface-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the temp directory is writable");
        std::fs::write(dir.join("app.sln"), body).expect("the temp directory is writable");

        TempSolution(dir)
    }

    /// Absolute path to `app.sln`.
    fn sln(&self) -> PathBuf {
        self.0.join("app.sln")
    }

    /// The directory itself, used as `--root` so `.msvclsp/` lands beside the solution.
    fn root(&self) -> &Path {
        &self.0
    }

    /// Write `.msvclsp/state.json` directly, for tests that need a pre-recorded selection without
    /// a working `generate`/`set_active_config` CLI path to produce one.
    fn seed_state(&self, contents: &str) {
        let msvclsp_dir = self.0.join(".msvclsp");
        std::fs::create_dir_all(&msvclsp_dir).expect("the temp directory is writable");
        std::fs::write(msvclsp_dir.join("state.json"), contents)
            .expect("the temp directory is writable");
    }
}

impl Drop for TempSolution {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A minimal but well-formed `.sln` body: the supported format-version header, one C++ project,
/// and the two solution-level pairs it declares, each mapped identically for the project so
/// `list-configs` has something real to report.
fn minimal_sln() -> String {
    r#"Microsoft Visual Studio Solution File, Format Version 12.00
Project("{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}") = "app", "app\app.vcxproj", "{A50E4C4B-9A9D-4B1A-8E2E-000000000001}"
EndProject
Global
    GlobalSection(SolutionConfigurationPlatforms) = preSolution
        Debug|x64 = Debug|x64
        Release|x64 = Release|x64
    EndGlobalSection
    GlobalSection(ProjectConfigurationPlatforms) = postSolution
        {A50E4C4B-9A9D-4B1A-8E2E-000000000001}.Debug|x64.ActiveCfg = Debug|x64
        {A50E4C4B-9A9D-4B1A-8E2E-000000000001}.Debug|x64.Build.0 = Debug|x64
        {A50E4C4B-9A9D-4B1A-8E2E-000000000001}.Release|x64.ActiveCfg = Release|x64
        {A50E4C4B-9A9D-4B1A-8E2E-000000000001}.Release|x64.Build.0 = Release|x64
    EndGlobalSection
EndGlobal
"#
    .to_string()
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

/// `generate` against a solution that does not exist must surface a structured error, not the
/// retired `Error::Unimplemented` placeholder — `solution-not-found`, exit code `3`, matching
/// `list_configs_against_a_missing_solution_is_a_structured_error`'s pattern. `generate` never
/// reaches toolchain discovery on this path, so it needs no real Visual Studio install to test.
#[test]
fn generate_against_a_missing_solution_emits_an_envelope_under_json() {
    let output = run(&[
        "generate",
        "missing.sln",
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--json",
    ]);

    assert_eq!(code(&output), 3);
    // Asserted as an exact string rather than a parsed value: this pins the key set, the key
    // order, and the wire strings in one place, and needs no JSON parser in the test crate.
    assert_eq!(
        stdout(&output),
        concat!(
            r#"{"schema_version":1,"has_error":true,"result":null,"#,
            r#""error":{"kind":"solution-not-found","#,
            r#""message":"solution not found: missing.sln","causes":[]},"#,
            r#""warnings":[]}"#,
            "\n"
        )
    );
    // The human message is on stderr in both modes — MVP-28 folds it into the plugin's error.
    assert!(stderr(&output).contains("solution not found"));
}

#[test]
fn generate_against_a_missing_solution_leaves_stdout_empty_without_json() {
    let output = run(&[
        "generate",
        "missing.sln",
        "--configuration",
        "Debug",
        "--platform",
        "x64",
    ]);

    assert_eq!(code(&output), 3);
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("solution not found"));
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

/// The cases above only ever hit `missing.sln`, which never reaches `sln::parse_sln`. This is the
/// reachable path most likely to leak a stray `print!` — real parsing, real file I/O, real
/// `Store::open` — so it gets its own one-object assertion against a solution that actually
/// exists.
#[test]
fn json_stdout_is_one_object_for_a_real_solution() {
    let solution = TempSolution::new("one-object-real-solution", &minimal_sln());
    let sln_str = solution.sln();
    let sln_str = sln_str.to_str().expect("temp path is valid UTF-8");
    let root_str = solution.root().to_str().expect("temp path is UTF-8");

    let args = ["list-configs", sln_str, "--root", root_str, "--json"];
    assert_stdout_is_one_json_object(&run(&args), &format!("{args:?}"));
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

/// MVP-13 added a second severity, so this owes an assertion on the wire string too — the
/// classification is a contract with the plugin, and a level that never leaves the library is
/// indistinguishable from not having split it.
#[test]
fn warnings_ride_in_the_envelope() {
    let missing_header = TempSolution::new(
        "missing-header",
        "Project(\"{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}\") = \"app\", \"app\\app.vcxproj\", \
         \"{A50E4C4B-9A9D-4B1A-8E2E-000000000001}\"\nEndProject\n",
    );
    let sln_path = missing_header.sln();
    let sln_str = sln_path.to_str().expect("temp path is valid UTF-8");
    let root_str = missing_header.root().to_str().expect("temp path is UTF-8");

    let output = run(&["list-configs", sln_str, "--root", root_str, "--json"]);

    assert_eq!(code(&output), 0);
    let out = stdout(&output);
    assert!(out.contains(r#""severity":"warning""#), "stdout was: {out}");
    assert!(
        out.contains("no `Format Version` header"),
        "stdout was: {out}"
    );
    assert!(
        stderr(&output).contains("warning:") && stderr(&output).contains("no `Format Version`"),
        "stderr was: {}",
        stderr(&output)
    );

    let unexpected_version = TempSolution::new(
        "unexpected-version",
        "Microsoft Visual Studio Solution File, Format Version 11.00\n\
         Project(\"{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}\") = \"app\", \"app\\app.vcxproj\", \
         \"{A50E4C4B-9A9D-4B1A-8E2E-000000000001}\"\nEndProject\n",
    );
    let sln_path = unexpected_version.sln();
    let sln_str = sln_path.to_str().expect("temp path is valid UTF-8");
    let root_str = unexpected_version
        .root()
        .to_str()
        .expect("temp path is UTF-8");

    let output = run(&["list-configs", sln_str, "--root", root_str, "--json"]);

    assert_eq!(code(&output), 0);
    let out = stdout(&output);
    assert!(out.contains(r#""severity":"note""#), "stdout was: {out}");
    assert!(out.contains("11.00"), "stdout was: {out}");
    assert!(
        stderr(&output).contains("note:") && stderr(&output).contains("11.00"),
        "stderr was: {}",
        stderr(&output)
    );
}

/// The end-to-end path through `sln::parse_sln`, replacing the placeholder that stood in for it
/// while `list-configs` returned `Error::Unimplemented`. `available` must list both declared
/// pairs in declaration order, and `active` must be `null` — no `.msvclsp/state.json` exists yet
/// for this solution.
#[test]
fn list_configs_reports_available_configurations_with_no_active_selection() {
    let solution = TempSolution::new("available-no-active", &minimal_sln());
    let sln_str = solution.sln();
    let sln_str = sln_str.to_str().expect("temp path is valid UTF-8");
    let root_str = solution.root().to_str().expect("temp path is UTF-8");

    let output = run(&["list-configs", sln_str, "--root", root_str, "--json"]);

    assert_eq!(code(&output), 0);
    let stdout = stdout(&output);
    assert!(stdout.contains(r#""configuration":"Debug","platform":"x64""#));
    assert!(stdout.contains(r#""configuration":"Release","platform":"x64""#));
    assert!(stdout.contains(r#""active":null"#), "stdout was: {stdout}");
    // Declaration order: Debug|x64 named before Release|x64 in `minimal_sln`.
    let debug_index = stdout.find(r#""configuration":"Debug""#).unwrap();
    let release_index = stdout.find(r#""configuration":"Release""#).unwrap();
    assert!(
        debug_index < release_index,
        "available pairs were not in declaration order: {stdout}"
    );
}

/// Proves `list-configs` actually consults `db::Store` rather than always reporting `active:
/// null` — `.msvclsp/state.json` is seeded directly (matching the shape `db.rs`'s own tests
/// write and round-trip) since no working `generate`/`set_active_config` CLI path exists yet to
/// produce one.
#[test]
fn list_configs_reports_the_last_used_selection() {
    let solution = TempSolution::new("last-used-selection", &minimal_sln());
    // Keyed by the solution's path relative to `--root`, lowercased (ADR 20) — the solution sits
    // directly under root here, so the key is just its lowercased filename.
    solution.seed_state(
        r#"{
            "version": 1,
            "solutions": {
                "app.sln": {
                    "known_configs": [],
                    "active_config": {"configuration": "Release", "platform": "x64"},
                    "files": {}
                }
            }
        }"#,
    );

    let sln_str = solution.sln();
    let sln_str = sln_str.to_str().expect("temp path is valid UTF-8");
    let root_str = solution.root().to_str().expect("temp path is UTF-8");

    let output = run(&["list-configs", sln_str, "--root", root_str, "--json"]);

    assert_eq!(code(&output), 0);
    let stdout = stdout(&output);
    assert!(
        stdout.contains(r#""active":{"configuration":"Release","platform":"x64"}"#),
        "stdout was: {stdout}"
    );
}

/// `list-configs` against a solution that does not exist must surface a structured,
/// non-`Unimplemented` error now that the handler actually runs — `solution-not-found`, exit
/// code `3` (ADR 02's table), not the placeholder `unimplemented`/`9` this path returned before
/// MVP-21.
#[test]
fn list_configs_against_a_missing_solution_is_a_structured_error() {
    let output = run(&["list-configs", "no-such-solution.sln", "--json"]);

    assert_eq!(code(&output), 3);
    assert_eq!(
        stdout(&output),
        concat!(
            r#"{"schema_version":1,"has_error":true,"result":null,"#,
            r#""error":{"kind":"solution-not-found","#,
            r#""message":"solution not found: no-such-solution.sln","causes":[]},"#,
            r#""warnings":[]}"#,
            "\n"
        )
    );
    assert!(stderr(&output).contains("solution not found"));
}

/// `generate` against a configuration/platform pair the solution never declares must fail with
/// `Error::InvalidConfiguration` (exit code `7`) listing the valid pairs, reachable via the real
/// binary since validation happens before toolchain discovery — no Visual Studio install needed.
#[test]
fn generate_against_an_invalid_configuration_is_a_structured_error() {
    let solution = TempSolution::new("invalid-configuration", &minimal_sln());
    let sln_str = solution.sln();
    let sln_str = sln_str.to_str().expect("temp path is valid UTF-8");

    let output = run(&[
        "generate",
        sln_str,
        "--configuration",
        "Release",
        "--platform",
        "ARM64",
        "--json",
    ]);

    assert_eq!(code(&output), 7);
    let out = stdout(&output);
    assert!(
        out.contains(r#""kind":"invalid-configuration""#),
        "stdout was: {out}"
    );
    assert!(out.contains("Debug|x64"), "stdout was: {out}");
    assert!(out.contains("Release|x64"), "stdout was: {out}");
    assert!(
        stderr(&output).contains("is not a configuration of this solution"),
        "stderr was: {}",
        stderr(&output)
    );
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
