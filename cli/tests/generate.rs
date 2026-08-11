//! Fixture-driven integration tests, and — until `fixtures/simple` exists (MVP-23) — tests for
//! `generate`'s own pipeline behavior against a hand-authored temp-tree project.
//!
//! The golden-file tests below stay `#[ignore]`d placeholders: `fixtures/` is still README-only
//! stubs, and per ADR 23's own discussion, authoring a real fixture here would be under-scoped
//! for what MVP-23 commits to (source files real enough for manual `clangd` verification) and
//! would either block this issue or get thrown away.
//!
//! Everything else here calls [`msvclsp::cli::run`] directly with a [`FakeToolchain`] rather
//! than spawning the compiled binary — `generate` now discovers a real toolchain to evaluate
//! projects, and nothing in a subprocess can have that swapped out from outside. See
//! [ADR 24](../../docs/adr/24-generate-wiring.md).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use clap::Parser;

use msvclsp::cli::{Cli, Outcome};
use msvclsp::toolchain::{FakeToolchain, Toolchain};
use msvclsp::{Reporter, Result};

#[test]
#[ignore = "not yet implemented"]
fn simple_solution_matches_golden() {
    todo!("parse fixtures/simple, compare against tests/golden/simple.json")
}

#[test]
#[ignore = "not yet implemented"]
fn props_inheritance_matches_golden() {
    todo!("parse fixtures/props-inheritance, compare against golden")
}

/// A GUID stable across every `TempProject`; nothing here needs more than one project, so there
/// is nothing for a collision to confuse.
const PROJECT_GUID: &str = "{A50E4C4B-9A9D-4B1A-8E2E-000000000001}";

/// A hand-authored `app.sln` + `App/App.vcxproj` + `App/src/main.cpp` tree, real enough for a
/// genuine `msbuild::evaluate` pass under `FakeToolchain` — sibling to `cli_surface.rs`'s own
/// `TempSolution`, per ADR 23's precedent against sharing test helpers across the
/// lib/integration-test boundary, extended here to write an evaluable project rather than a bare
/// `.sln`. The project XML needs no `<Import>` of the real Visual Studio targets, matching
/// `msbuild.rs`'s own unit-test fixtures.
struct TempProject(PathBuf);

impl TempProject {
    /// `build` controls whether the sole `Debug|x64` mapping carries a `.Build.0` row — see
    /// `an_unbuilt_project_is_still_indexed`.
    fn new(name: &str, build: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let dir = std::env::temp_dir().join(format!(
            "msvclsp-generate-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("App/src")).expect("the temp directory is writable");

        let build_row = if build {
            format!("        {PROJECT_GUID}.Debug|x64.Build.0 = Debug|x64\n")
        } else {
            String::new()
        };
        std::fs::write(
            dir.join("app.sln"),
            format!(
                r#"Microsoft Visual Studio Solution File, Format Version 12.00
Project("{{8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942}}") = "App", "App\App.vcxproj", "{PROJECT_GUID}"
EndProject
Global
    GlobalSection(SolutionConfigurationPlatforms) = preSolution
        Debug|x64 = Debug|x64
    EndGlobalSection
    GlobalSection(ProjectConfigurationPlatforms) = postSolution
        {PROJECT_GUID}.Debug|x64.ActiveCfg = Debug|x64
{build_row}    EndGlobalSection
EndGlobal
"#
            ),
        )
        .expect("the temp directory is writable");

        std::fs::write(
            dir.join("App/App.vcxproj"),
            r#"<Project>
  <ItemGroup Label="ProjectConfigurations">
    <ProjectConfiguration Include="Debug|x64">
      <Configuration>Debug</Configuration>
      <Platform>x64</Platform>
    </ProjectConfiguration>
  </ItemGroup>
  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>APP;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemGroup>
    <ClCompile Include="src\main.cpp" />
  </ItemGroup>
</Project>
"#,
        )
        .expect("the temp directory is writable");

        std::fs::write(dir.join("App/src/main.cpp"), "int main() { return 0; }\n")
            .expect("the temp directory is writable");

        TempProject(dir)
    }

    fn sln(&self) -> PathBuf {
        self.0.join("app.sln")
    }

    fn vcxproj(&self) -> PathBuf {
        self.0.join("App/App.vcxproj")
    }

    /// Force a file's mtime forward, matching `cli/src/testing.rs::TempTree::set_mtime` and
    /// `db.rs`'s own staleness tests. Two successive writes can land within the same filesystem
    /// timestamp tick, which would make a "touch and expect staleness" test flaky without this.
    /// Staleness is mtime-based, so content is left untouched — this must work on `.vcxproj`
    /// files too, and injecting arbitrary text would corrupt their XML.
    fn touch(&self, relative: &str) {
        let path = self.0.join(relative);
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("the temp tree file exists and is writable");
        file.set_modified(SystemTime::now() + Duration::from_secs(60))
            .expect("set_modified is supported on the test platform");
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic toolchain for every test here — nothing asserts on its contents, only that
/// evaluation completes, so its paths need not exist on disk.
fn fake_toolchain() -> FakeToolchain {
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

fn generate(sln: &Path) -> Result<Outcome> {
    let cli = Cli::try_parse_from([
        "msvclsp",
        "generate",
        sln.to_str().expect("temp path is valid UTF-8"),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
    ])
    .expect("parses");
    let mut reporter = Reporter::new(false);
    msvclsp::cli::run(&cli, &mut reporter, &fake_toolchain())
}

fn generate_forced(sln: &Path) -> Result<Outcome> {
    let cli = Cli::try_parse_from([
        "msvclsp",
        "generate",
        sln.to_str().expect("temp path is valid UTF-8"),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--force",
    ])
    .expect("parses");
    let mut reporter = Reporter::new(false);
    msvclsp::cli::run(&cli, &mut reporter, &fake_toolchain())
}

fn database_path(outcome: Outcome) -> (PathBuf, bool) {
    match outcome {
        Outcome::Generate(result) => (result.database, result.regenerated),
        Outcome::ListConfigs(_) => panic!("expected a Generate outcome"),
    }
}

#[test]
fn a_second_run_with_no_changes_is_a_no_op() {
    let project = TempProject::new("no_op", true);

    let (database, first_regenerated) =
        database_path(generate(&project.sln()).expect("first generation succeeds"));
    assert!(
        first_regenerated,
        "the first run has nothing to cache against"
    );
    let first_written = std::fs::metadata(&database)
        .expect("compile_commands.json exists")
        .modified()
        .expect("mtime is readable");

    let (_, second_regenerated) =
        database_path(generate(&project.sln()).expect("second generation succeeds"));
    let second_written = std::fs::metadata(&database)
        .expect("compile_commands.json still exists")
        .modified()
        .expect("mtime is readable");

    assert!(
        !second_regenerated,
        "nothing changed, so this must be a no-op"
    );
    assert_eq!(
        first_written, second_written,
        "a no-op run must not rewrite compile_commands.json"
    );
}

#[test]
fn force_overrides_an_up_to_date_cache() {
    let project = TempProject::new("force", true);

    database_path(generate(&project.sln()).expect("first generation succeeds"));
    let (_, forced_regenerated) =
        database_path(generate_forced(&project.sln()).expect("forced generation succeeds"));

    assert!(
        forced_regenerated,
        "--force must regenerate even though nothing changed"
    );
}

#[test]
fn touching_the_vcxproj_invalidates_the_cache() {
    let project = TempProject::new("touch_vcxproj", true);

    database_path(generate(&project.sln()).expect("first generation succeeds"));
    project.touch("App/App.vcxproj");
    let (_, regenerated) =
        database_path(generate(&project.sln()).expect("second generation succeeds"));

    assert!(
        regenerated,
        "a touched project file must invalidate the cache"
    );
}

/// The dependency set recorded by a successful `generate` must include the `.sln` itself, not
/// only the `.vcxproj` files it maps — otherwise editing the solution (adding a project, a new
/// configuration) would go unnoticed by the staleness cache.
#[test]
fn touching_the_solution_invalidates_the_cache() {
    let project = TempProject::new("touch_sln", true);

    database_path(generate(&project.sln()).expect("first generation succeeds"));
    project.touch("app.sln");
    let (_, regenerated) =
        database_path(generate(&project.sln()).expect("second generation succeeds"));

    assert!(
        regenerated,
        "a touched solution file must invalidate the cache"
    );
}

/// ADR 24's decision: a project mapped to the requested configuration but unchecked in
/// Configuration Manager (`Build.0` absent) is still evaluated and indexed, since its files
/// still need real completions/diagnostics when a user opens them.
#[test]
fn an_unbuilt_project_is_still_indexed() {
    let project = TempProject::new("unbuilt", false);

    let (database, _) = database_path(generate(&project.sln()).expect("generation succeeds"));
    let contents = std::fs::read_to_string(&database).expect("compile_commands.json exists");

    assert!(
        contents.contains("main.cpp"),
        "an unbuilt project's file must still appear in the database: {contents}"
    );
}

/// Not one of `Store`'s own unit tests (those exercise `record_dependencies`/`is_up_to_date` in
/// isolation) — this proves `generate`'s own wiring reaches them at all with the project's real
/// evaluation output, the same kind of proof `list_configs_propagates_a_missing_solution` gives
/// for `list-configs` reaching `sln::parse_sln`.
#[test]
fn a_vcxproj_edit_is_visible_in_the_regenerated_database() {
    let project = TempProject::new("regenerated_contents", true);

    database_path(generate(&project.sln()).expect("first generation succeeds"));
    std::fs::write(
        project.vcxproj(),
        r#"<Project>
  <ItemGroup Label="ProjectConfigurations">
    <ProjectConfiguration Include="Debug|x64">
      <Configuration>Debug</Configuration>
      <Platform>x64</Platform>
    </ProjectConfiguration>
  </ItemGroup>
  <ItemDefinitionGroup>
    <ClCompile>
      <PreprocessorDefinitions>CHANGED;%(PreprocessorDefinitions)</PreprocessorDefinitions>
    </ClCompile>
  </ItemDefinitionGroup>
  <ItemGroup>
    <ClCompile Include="src\main.cpp" />
  </ItemGroup>
</Project>
"#,
    )
    .expect("the temp tree file is writable");
    let file = std::fs::File::options()
        .write(true)
        .open(project.vcxproj())
        .expect("the temp tree file exists and is writable");
    file.set_modified(SystemTime::now() + Duration::from_secs(60))
        .expect("set_modified is supported on the test platform");

    let (database, regenerated) =
        database_path(generate(&project.sln()).expect("second generation succeeds"));
    let contents = std::fs::read_to_string(&database).expect("compile_commands.json exists");

    assert!(regenerated, "the edit must invalidate the cache");
    assert!(
        contents.contains("/DCHANGED"),
        "the regenerated database must reflect the edit: {contents}"
    );
}
