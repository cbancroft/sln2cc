# Overview

This document contains the initial brainstorming of what the MSVC LSP is trying to accomplish.

# Goals

My goals for this project are:

1. Learn the fundamentals of how LSP's work
2. Learn Rust by implementing an interesting project.
3. Build an MVP showing it is possible to do code completion based on MS Visual Studio solutions

# Problem Statement

This app will be a prototype to explore learning Rust as well as creating something I wish I had while
I was at Bethesda. I prefer to use Neovim for most of my development work but it is hard to impossible
to use it for projects managed by a Visual Studio C++ solution, as there is no existing LSP or solution
that feeds existing LSPs the compilation data. Previously I had to convert all solutions to CMake projects
and use those to generate a code database that the clangd LSP could use. This is not really feasible to do
for every project I work on as it is a highly manual process that I kept relying on AI to do for me. 

# Milestones

This project should be able to accomplish the following:

1. Parse MSVC solution, project and properties files
2. Extract all compilation flags from the solution
3. Construct a list of files managed by the solution/projects
4. Store relevant information in a local file (maybe a sqlite database or a flat file?)
5. Set up a channel for LSP requests and responses
6. Parse configuration data from a local configuration file -- not sure what will be stored in here yet
7. Locate a solution root, or have one specified via environment or command line on startup
8. Feed files and compilation flag information to an instance of clangd to do the actual management of LSP functionality to start


## Minimum Viable Product
- Basic Rust binary building
- Responds to core set of LSP queries with dummy responses to start
- XML parser that can parse a MSVC solution, find its projects and properties lists, and extract all code files, compilers and compilation flags
- Parses a basic configuration file
- Can be passed a solution file to process via environment, configuration file, or by finding a solution root
- Stores compilation database information within a sqlite database
    - This database should store modification timestamps and only reparse the solution/files when an change is seen
- Create a Neovim plugin/configuration that allows for the new LSP to be started and used for cpp, c, and asm files
- Basic user documentation

## v1
- Starts a child clangd process that it proxies LSP requests to
- Feeds all relevant parsed compilation database information to clangd
- Proxies LSP requests to clangd and generates responses


# Technology Stack

- Rust -- Implementation of the LSP will be done in Rust. Investigate applicable libraries that can be used for implementing LSP command
  handling, sqlite integration and XML parsing.
    - All functionality should be properly unit tested
    - There should be sample configuration data to properly execute integration/end-to-end tests. This should include test LSP clients, sample MSVC files of
        varying complexity, and code files.

- Neovim -- The primary LSP client used for my own testing will be Neovim

