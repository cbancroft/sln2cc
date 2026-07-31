# Project Spec

This document defines the purpose of MSVC LSP: who it is for, what problem it solves, and what
the project does. It is derived from the initial thinking in [brainstorm.md](brainstorm.md) and
will be expanded in future sessions.

Decisions about product behavior and architecture — including ones that reshape what is described
here — are recorded as architecture decision records under [adr/](adr/), one file per decision, each
tied to the [todos.md](todos.md) item it came out of. This document states what the product does;
the ADRs state why it does that rather than the alternative. Conventions for writing them are in
section 9 of [../CLAUDE.md](../CLAUDE.md).

# Who This Is For

Developers who:
- Work on C/C++/ASM codebases managed by Visual Studio solutions (`.sln`) and projects (`.vcxproj`).
- Prefer to use an editor other than Visual Studio for day-to-day work — Neovim, specifically, for
  the primary author.
- Want modern LSP tooling (completion, diagnostics, go-to-definition, etc.) via `clangd`, but are
  blocked because nothing translates MSVC's project format into the compilation database `clangd`
  expects.

# Problem Statement

`clangd` provides high-quality C/C++ language support, but it needs a `compile_commands.json`
compilation database to know how each file is actually compiled (include paths, defines, language
standard, etc.). Projects built with CMake can generate this automatically. Projects built with
Visual Studio solutions cannot — there is no existing tool that reads `.sln`/`.vcxproj`/`.props`
files and produces an equivalent compilation database.

The only current workaround is manually converting a solution to CMake to generate a database,
which is a labor-intensive, error-prone, per-project process that doesn't scale across the many
solutions a developer might touch, and doesn't stay in sync as the solution changes.

# What The Project Does

MSVC LSP closes that gap with two artifacts that work together:

1. **A Rust CLI binary** that parses a Visual Studio solution and its associated project and
   property files for a given configuration/platform (e.g. `Debug|x64`), and generates a standard
   `compile_commands.json` that `clangd` can consume directly. It also:
   - Discovers the available configuration/platform combinations in a solution so the caller can
     choose one.
   - Persists parsed state and the active configuration/platform selection locally, so it can
     detect when the solution or its files have changed and avoid unnecessary reparsing.

2. **A Neovim plugin** that makes the binary practical to use day-to-day: installing/running it,
   generating and regenerating the compilation database, switching the active configuration/
   platform, restarting `clangd` with the new database, and exposing sensible keymaps and
   configuration options for all of the above. It also:
   - Provides Lua functionality to locate `.sln` solution files within the project directory and
     its subdirectories, rather than requiring the user to specify one manually.
   - Integrates with existing Neovim project/workspace conventions so solution discovery and
     `clangd` startup fit naturally into how the user already organizes projects.
   - Launches `clangd` pointed at an explicit `compile_commands.json`, rather than relying on
     `clangd`'s own directory-walking discovery.

Generated artifacts — the `compile_commands.json` and the local database — are stored in a
`.msvclsp` directory at the project root.

Together, these let a developer point Neovim at a Visual Studio solution and get working
`clangd`-powered language support, without hand-converting the project to CMake or manually
maintaining a compilation database.

# Multiple Solution Handling

A single project directory tree can contain more than one `.sln` file. For v1, only one solution
is **active** at a time per workspace, but the underlying data model is kept scoped per-solution
from the start so that supporting multiple simultaneously-active solutions later doesn't require a
rework.

- **Discovery is cached, not repeated on every action.** The plugin scans the directory tree for
  `.sln` files once and caches the results, rather than re-walking the tree on every command. A
  manual rescan command lets the user refresh the list when solutions are added, moved, or removed.
  If no cache exists yet when the user asks to select or switch a solution, discovery runs
  automatically inline before presenting the picker — one seamless action, not two separate steps.
- **Selection is an interactive picker, remembered per workspace.** When more than one solution is
  discovered, the plugin presents a picker (e.g. `vim.ui.select`) to choose the active one. That
  choice is persisted so future Neovim sessions in the same workspace don't re-prompt. An explicit
  "switch solution" command lets the user change the active solution at any time.
- **Workspace identity is the project root**, detected using the same root markers Neovim's LSP
  clients already use (e.g. `.git`), rather than a bespoke convention. This is also where `.msvclsp`
  lives.
- **Storage is a single `.msvclsp` at project root, namespaced by solution** — e.g.
  `.msvclsp/<solution-name>/compile_commands.json` — rather than one `.msvclsp` per solution
  directory. This keeps a previously-used solution's cached artifacts around when switching back to
  it later.
- **Configuration/platform is remembered per solution, independently.** Different solutions can
  define different configuration/platform names, so switching back to a previously-used solution
  restores that solution's own last-used configuration/platform rather than falling back to a
  single global value.

# Goals

- Learn the fundamentals of how LSPs work.
- Learn Rust by implementing a real, non-trivial project.
- Prove that accurate code completion/navigation is achievable for MSVC-solution-based codebases
  from an editor other than Visual Studio.

# Open Questions / To Be Refined

- Exact scope of MSBuild evaluation needed (property sheet inheritance, conditions, macros) to
  produce accurate flags for real-world solutions.
- How MSVC toolset/Windows SDK include paths and versions are discovered.
- CLI subcommand surface and output contract between the binary and the Neovim plugin.
- Local configuration file format and contents.
- What happens when the remembered/active solution's `.sln` file no longer exists on disk (moved
  or deleted)?
- Should there be a way to explicitly pin/override the active solution via config (e.g. for CI or
  scripting use), bypassing the interactive picker?
- Naming collisions: how should `.msvclsp/<solution-name>/` namespacing handle two discovered
  solutions that share the same filename in different directories?
