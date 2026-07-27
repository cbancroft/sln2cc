//! Fixture-driven integration tests.
//!
//! Each fixture under `../../fixtures/` is parsed with a `FakeToolchain` injected, and the
//! generated compilation database is compared against the matching golden file in
//! `tests/golden/`. The fake toolchain is what keeps those goldens byte-stable across
//! machines with different Visual Studio installations.

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
