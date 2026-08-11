//! Shared by every fixture-driven golden-file test — `generate.rs`'s `simple_solution_matches_
//! golden` today, and whatever MVP-24 adds for `multi-project`/`props-inheritance`/`multi-config`.
//!
//! A subdirectory carrying `mod.rs` rather than a bare `tests/support.rs` file, so cargo does not
//! treat it as a third top-level test binary — the standard Rust idiom for code shared *within*
//! the integration-test crate. This is a different concern from ADR 23's rejection of exposing
//! `cli/src/testing.rs::TempTree` here: that was about not leaking `pub(crate)` lib internals
//! across the lib/integration-test boundary, not about sharing code within `cli/tests/` itself.
//! See [ADR 25](../../../docs/adr/25-fixture-simple-and-golden-harness.md).

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::Parser;

use msvclsp::Reporter;
use msvclsp::cli::{Cli, Outcome};
use msvclsp::toolchain::{FakeToolchain, Toolchain};

/// Set to rewrite a golden file with fresh output instead of comparing against it.
///
/// Always still runs under [`fake_toolchain`], so the rewritten file stays reproducible — this
/// is for the one-time-by-hand case where a flag genuinely changes, not a way to point the suite
/// at a real toolchain.
const UPDATE_GOLDEN_ENV: &str = "MSVCLSP_UPDATE_GOLDEN";

/// Deterministic toolchain for every fixture-driven test. Empty `include_paths` keeps toolchain
/// system includes out of golden output entirely, so goldens only need to account for what the
/// fixture's own project settings produce.
pub fn fake_toolchain() -> FakeToolchain {
    FakeToolchain(Toolchain {
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

/// A directory removed when the test that made it ends — the `--root` a golden test points
/// `generate` at, so `cargo test` never writes a `.msvclsp/` directory inside `fixtures/` itself.
pub struct TempRoot(PathBuf);

impl TempRoot {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let path = env::temp_dir().join(format!(
            "msvclsp-golden-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the temp directory is writable");
        TempRoot(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `generate` for `fixture_sln`'s `Debug|x64` under [`fake_toolchain`], and either compare
/// the normalized result against `golden_path` or — under `MSVCLSP_UPDATE_GOLDEN=1` — rewrite it.
///
/// `fixture_sln` must be an absolute, dot-free path (see `generate.rs`'s callers, which build one
/// from `CARGO_MANIFEST_DIR` via `.parent()` rather than a `..` join) — the normalization below
/// matches it against `compile_commands.json`'s own paths by exact substring, which only works if
/// both sides went through the same lexical resolution.
pub fn assert_matches_golden(fixture_sln: &Path, golden_path: &Path) {
    let root = TempRoot::new("simple");
    let cli = Cli::try_parse_from([
        "msvclsp",
        "generate",
        fixture_sln.to_str().expect("fixture path is valid UTF-8"),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--root",
        root.path().to_str().expect("temp root is valid UTF-8"),
    ])
    .expect("parses");
    let mut reporter = Reporter::new(false);
    let outcome = msvclsp::cli::run(&cli, &mut reporter, &fake_toolchain())
        .expect("generation against a fixture succeeds");

    let database = match outcome {
        Outcome::Generate(result) => result.database,
        Outcome::ListConfigs(_) => panic!("expected a Generate outcome"),
    };
    let actual = std::fs::read_to_string(&database).expect("compile_commands.json was written");

    let fixture_dir = fixture_sln
        .parent()
        .expect("a fixture .sln has a parent directory");
    let mut normalized = normalize(&actual, fixture_dir);
    normalized.push('\n');

    if env::var_os(UPDATE_GOLDEN_ENV).is_some() {
        if let Some(parent) = golden_path.parent() {
            std::fs::create_dir_all(parent).expect("the golden directory is writable");
        }
        std::fs::write(golden_path, &normalized).expect("the golden file is writable");
        return;
    }

    let expected = std::fs::read_to_string(golden_path).unwrap_or_else(|_| {
        panic!(
            "{} does not exist yet; run with {UPDATE_GOLDEN_ENV}=1 to create it",
            golden_path.display()
        )
    });
    // A checked-out golden file may carry CRLF line endings depending on the machine's git
    // autocrlf setting; `normalized` never does, since it is built entirely in memory.
    assert_eq!(
        normalized,
        expected.replace("\r\n", "\n"),
        "{} no longer matches its golden file. If this is a legitimate change, rerun with \
         {UPDATE_GOLDEN_ENV}=1 and review the diff before committing.",
        fixture_sln.display()
    );
}

/// Replace every JSON-escaped path separator with `/` and the fixture's own absolute directory
/// with a `<FIXTURE>` placeholder, so the checked-in golden file is identical across machines,
/// checkout locations, and Windows/Linux. Safe because a path separator is the only place this
/// crate's own tests ever produce a backslash — never in a define, flag, or directory name.
fn normalize(json: &str, fixture_dir: &Path) -> String {
    let slashed = json.replace("\\\\", "/");
    let fixture_display = fixture_dir.display().to_string().replace('\\', "/");
    slashed.replace(&fixture_display, "<FIXTURE>")
}
