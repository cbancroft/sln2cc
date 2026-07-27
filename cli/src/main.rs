//! Thin entry point. Argument parsing and dispatch only — see `lib.rs` for the real work.

fn main() {
    // TODO: replace with a proper subcommand parser once a CLI crate is chosen.
    // Planned subcommands (see docs/engineering_spec.md):
    //   discover      find .sln files under a project root
    //   list-configs  list a solution's configuration/platform options
    //   generate      emit compile_commands.json for a solution + config/platform
    eprintln!("msvclsp: not yet implemented");
    std::process::exit(1);
}
