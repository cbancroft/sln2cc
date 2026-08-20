//! Fixture-driven integration tests, plus tests for `generate`'s own pipeline behavior against a
//! hand-authored temp-tree project.
//!
//! Everything here calls [`sln2cc::cli::run`] directly with a [`sln2cc::toolchain::
//! FakeToolchain`] rather than spawning the compiled binary — `generate` now discovers a real
//! toolchain to evaluate projects, and nothing in a subprocess can have that swapped out from
//! outside. See [ADR 24](../../docs/adr/24-generate-wiring.md).

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use clap::Parser;

use sln2cc::cli::{Cli, Outcome};
use sln2cc::{Reporter, Result};

/// `fixtures/<name>`, built from `.parent()` rather than a `..` join so the result carries no
/// literal `..` component — `support::assert_matches_golden` matches this path against
/// `compile_commands.json`'s own (fully lexically resolved) paths by exact substring, which only
/// works if this one is equally clean.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent directory")
        .join("fixtures")
        .join(name)
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

#[test]
fn simple_solution_matches_golden() {
    support::assert_matches_golden(
        &fixture("simple").join("simple.sln"),
        &golden("simple.json"),
    );
}

#[test]
fn property_group_import_order_matches_golden() {
    // MVP-44: a self-defaulting property from one import, a local unconditional override
    // sandwiched between it and a second import, and that second import's own Condition gated on
    // the result — end to end through items.rs's settling and flags.rs's translation, which the
    // imports.rs unit test alone doesn't exercise.
    support::assert_matches_golden(
        &fixture("property-group-import-order").join("property-group-import-order.sln"),
        &golden("property-group-import-order.json"),
    );
}

#[test]
fn item_definition_order_matches_golden() {
    // MVP-45: a source `ItemGroup` declared before the `ItemDefinitionGroup`s that apply to it —
    // the real external project's shape, and the reverse of what every other fixture does. See
    // ADR 40.
    support::assert_matches_golden(
        &fixture("item-definition-order").join("item-definition-order.sln"),
        &golden("item-definition-order.json"),
    );
}

#[test]
fn props_inheritance_matches_golden() {
    support::assert_matches_golden(
        &fixture("props-inheritance").join("props-inheritance.sln"),
        &golden("props-inheritance.json"),
    );
}

#[test]
fn multi_project_matches_golden() {
    support::assert_matches_golden(
        &fixture("multi-project").join("multi-project.sln"),
        &golden("multi-project.json"),
    );
}

#[test]
fn multi_config_matches_golden() {
    support::assert_matches_golden(
        &fixture("multi-config").join("multi-config.sln"),
        &golden("multi-config.json"),
    );
}

#[test]
fn makefile_configuration_matches_golden() {
    // V2-06: a ConfigurationType=Makefile project (a vendored NMake dependency, e.g. Detours)
    // must get real flags for its own ClCompile item, read from its NMake* properties rather
    // than the ItemDefinitionGroup pipeline an unimported toolchain chain never populates. See
    // ADR 42 and ADR 52.
    support::assert_matches_golden(
        &fixture("makefile-configuration").join("makefile-configuration.sln"),
        &golden("makefile-configuration.json"),
    );
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
            "sln2cc-generate-{}-{name}-{}",
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

fn generate(sln: &Path) -> Result<Outcome> {
    let cli = Cli::try_parse_from([
        "sln2cc",
        "generate",
        sln.to_str().expect("temp path is valid UTF-8"),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
    ])
    .expect("parses");
    let mut reporter = Reporter::new(false);
    sln2cc::cli::run(&cli, &mut reporter, &support::fake_toolchain())
}

fn generate_forced(sln: &Path) -> Result<Outcome> {
    let cli = Cli::try_parse_from([
        "sln2cc",
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
    sln2cc::cli::run(&cli, &mut reporter, &support::fake_toolchain())
}

/// Like [`generate`], but with an explicit `--root` rather than the default (the solution's own
/// directory) — the shape a real multi-solution workspace uses, and the only way to make two
/// independent [`TempProject`]s share one `.sln2cc` parent.
fn generate_with_root(sln: &Path, root: &Path) -> Result<Outcome> {
    let cli = Cli::try_parse_from([
        "sln2cc",
        "generate",
        sln.to_str().expect("temp path is valid UTF-8"),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--root",
        root.to_str().expect("temp path is valid UTF-8"),
    ])
    .expect("parses");
    let mut reporter = Reporter::new(false);
    sln2cc::cli::run(&cli, &mut reporter, &support::fake_toolchain())
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

/// `resolve_output_dir`'s collision-avoidance (ADR 19) is unit-tested in isolation in
/// `compdb.rs`, but nothing before this exercised it through the real `generate` pipeline with
/// two *different* solutions sharing one `.sln2cc` root — the actual multi-solution workspace
/// shape V1-02 describes. Closes the gap between that todo item's `Tests:` line and what existed.
#[test]
fn two_solutions_generate_into_distinct_directories() {
    let shared_root = std::env::temp_dir().join(format!(
        "sln2cc-generate-shared-root-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&shared_root);

    let first = TempProject::new("first", true);
    let second = TempProject::new("second", true);

    let (first_db, _) = database_path(
        generate_with_root(&first.sln(), &shared_root).expect("first solution generates"),
    );
    let (second_db, _) = database_path(
        generate_with_root(&second.sln(), &shared_root).expect("second solution generates"),
    );

    assert_ne!(
        first_db, second_db,
        "distinct solutions must not share an output directory"
    );
    assert!(first_db.starts_with(&shared_root));
    assert!(second_db.starts_with(&shared_root));

    let first_contents = std::fs::read_to_string(&first_db).expect("first database exists");
    let second_contents = std::fs::read_to_string(&second_db).expect("second database exists");
    assert!(
        first_contents.contains("first"),
        "first database must reference its own project's directory: {first_contents}"
    );
    assert!(
        second_contents.contains("second"),
        "second database must reference its own project's directory: {second_contents}"
    );
    assert!(
        !first_contents.contains("second") && !second_contents.contains("first"),
        "neither database may have been clobbered by the other solution's run"
    );

    let _ = std::fs::remove_dir_all(&shared_root);
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
