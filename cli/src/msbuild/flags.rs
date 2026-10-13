//! Settled `ClCompile` metadata, translated into `cl` flags.
//!
//! Only what changes how a file *parses* is translated: include directories, defines, the language
//! standard, and the switches that move a predefined macro. Codegen and link settings are skipped
//! outright — clangd never links, and a flag it cannot act on is one more thing to get wrong. What
//! is skipped, and why each one, is recorded at the bottom of this file.
//!
//! Flags stay in MSVC form and are consumed by clangd in clang-cl mode (CLAUDE.md §2), so nothing
//! here translates a switch to its native clang spelling. See
//! [ADR 12](../../../docs/adr/12-clcompile-flag-extraction.md).

use std::path::Path;

use crate::Reporter;
use crate::msbuild::FileCompilation;
use crate::msbuild::items::SourceItem;
use crate::msbuild::paths;

/// Turn one settled item into its compilation options.
///
/// Flags come out in the order of the table below, which is deterministic without sorting, since we
/// can't sort or that would mess up flags like `AdditionalOptions`.
pub fn compile(
    source: &SourceItem,
    project_dir: &Path,
    reporter: &mut Reporter,
) -> FileCompilation {
    let get = |name: &str| source.metadata.get(name).unwrap_or_default().trim();
    let mut flags = Vec::new();

    // A value still holding a sigil is something an earlier pass copied through vrebatim. It is
    // guaranteed garbage in a compile command and this is the place we can say no.
    for name in [
        "AdditionalIncludeDirectories",
        "PreprocessorDefinitions",
        "ForcedIncludeFiles",
        "AdditionalOptions",
    ] {
        let value = get(name);
        if value.contains("$(") || value.contains("%(") {
            reporter.warn_at(
                &source.declared_in,
                Some(source.line),
                format!(
                    "`{name}` still contains an unresolved reference and will reach clangd as written"
                    ));
        }
    }

    let include_paths = dedup(
        list(get("AdditionalIncludeDirectories"))
            .map(|entry| paths::resolve(project_dir, entry))
            .collect(),
    );

    let defines = dedup(
        list(get("PreprocessorDefinitions"))
            .map(str::to_string)
            .collect(),
    );

    for entry in list(get("UndefinePreprocessorDefinitions")) {
        flags.push(format!("/U{entry}"));
    }
    if get("UndefineAllPreprocessorDefinitions").eq_ignore_ascii_case("true") {
        flags.push("/u".to_string())
    }
    for entry in list(get("ForcedIncludeFiles")) {
        // Absolute: clangd resolves a `/FI` against its own working directory, not the project's.
        flags.push(format!(
            "/FI{}",
            paths::resolve(project_dir, entry).display()
        ));
    }

    // One entry per setting whose value is an enumeration. An empty or `Default` value means
    // "whatever cl does", which is also whatever clang-cl does, so it emits nothing. An
    // unrecognized value is named rather than guessed at.
    let mut enumerated = |name: &str, table: &[(&str, &str)], flags: &mut Vec<String>| {
        let value = get(name);
        if value.is_empty() || value.eq_ignore_ascii_case("Default") {
            return;
        }

        match table
            .iter()
            .find(|(from, _)| value.eq_ignore_ascii_case(from))
        {
            // An empty mapping is a real value that maps to no flag, e.g. ExceptionHandling=false.
            Some((_, "")) => {}
            Some((_, flag)) => flags.push((*flag).to_string()),
            None => reporter.warn_at(
                &source.declared_in,
                Some(source.line),
                format!("`{name}` is set to `{value}`, which is not recognized. Ignored"),
            ),
        }
    };

    enumerated(
        "CompileAs",
        &[("CompileAsC", "/TC"), ("CompileAsCpp", "/TP")],
        &mut flags,
    );
    enumerated(
        "LanguageStandard",
        &[
            ("stdcpp14", "/std:c++14"),
            ("stdcpp17", "/std:c++17"),
            ("stdcpp20", "/std:c++20"),
            ("stdcpplatest", "/std:c++latest"),
        ],
        &mut flags,
    );

    enumerated(
        "LanguageStandard_C",
        &[("stdc11", "/std:c11"), ("stdc17", "/std:c17")],
        &mut flags,
    );

    enumerated(
        "RuntimeLibrary",
        &[
            ("MultiThreaded", "/MT"),
            ("MultiThreadedDebug", "/MTd"),
            ("MultiThreadedDLL", "/MD"),
            ("MultiThreadedDebugDLL", "/MDd"),
        ],
        &mut flags,
    );
    enumerated(
        "ExceptionHandling",
        &[
            ("Sync", "/EHsc"),
            ("Async", "/EHa"),
            ("SyncCThrow", "/EHs"),
            ("false", ""),
        ],
        &mut flags,
    );

    enumerated(
        "EnableEnhancedInstructionSet",
        &[
            ("StreamingSIMDExtensions", "/arch:SSE"),
            ("StreamingSIMDExtensions2", "/arch:SSE2"),
            ("AdvancedVectorExtensions", "/arch:AVX"),
            ("AdvancedVectorExtensions2", "/arch:AVX2"),
            ("AdvancedVectorExtensions512", "/arch:AVX512"),
            ("NoExtensions", "/arch:IA32"),
        ],
        &mut flags,
    );

    enumerated(
        "StructMemberAlignment",
        &[
            ("1Byte", "/Zp1"),
            ("2Bytes", "/Zp2"),
            ("4Bytes", "/Zp4"),
            ("8Bytes", "/Zp8"),
            ("16Bytes", "/Zp16"),
        ],
        &mut flags,
    );

    enumerated(
        "CallingConvention",
        &[
            ("Cdecl", "/Gd"),
            ("FastCall", "/Gr"),
            ("StdCall", "/Gz"),
            ("VectorCall", "/Gv"),
        ],
        &mut flags,
    );
    enumerated(
        "FloatingPointModel",
        &[
            ("Precise", "/fp:precise"),
            ("Strict", "/fp:strict"),
            ("Fast", "/fp:fast"),
        ],
        &mut flags,
    );
    enumerated(
        "WarningLevel",
        &[
            ("TurnOffAllWarnings", "/W0"),
            ("Level1", "/W1"),
            ("Level2", "/W2"),
            ("Level3", "/W3"),
            ("Level4", "/W4"),
            ("EnableAllWarnings", "/Wall"),
        ],
        &mut flags,
    );

    boolean(
        &mut flags,
        get("ConformanceMode"),
        "/permissive-",
        "/permissive",
    );
    boolean(&mut flags, get("RuntimeTypeInfo"), "/GR", "/GR-");
    boolean(
        &mut flags,
        get("TreatWChar_tAsBuiltInType"),
        "/Zc:wchar_t",
        "/Zc:wchar_t-",
    );
    boolean(&mut flags, get("OpenMPSupport"), "/openmp", "");

    // Subtractive only, so it can quiet the editor but never make it noisier than the build
    for entry in list(get("DisableSpecificWarnings")) {
        flags.push(format!("/wd{entry}"))
    }

    // Last, so a hand-written switch overrides anything the table produced. Split using the
    // compiler's own tokenization rules (MVP-38, ADR 43) rather than whitespace, since this text
    // reaches cl.exe/clang-cl as literal command-line arguments.
    flags.extend(split_arguments(get("AdditionalOptions")));

    FileCompilation {
        file: source.path.clone(),
        include_paths,
        defines,
        flags,
    }
}

/// Split arguments the way `cl.exe` and `clang-cl`'s own command-line tokenizers do. Quoted
/// arguments will have spaces preserved, while properly handling escaped quotes.
fn split_arguments(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();

    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let mut arg = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            match c {
                _ if c.is_whitespace() && !quoted => break,
                '\\' => {
                    let mut backslashes = 0;
                    while chars.peek() == Some(&'\\') {
                        backslashes += 1;
                        chars.next();
                    }
                    if chars.peek() == Some(&'"') {
                        arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                        chars.next();
                        if backslashes % 2 == 1 {
                            arg.push('"');
                        } else {
                            quoted = !quoted;
                        }
                    } else {
                        arg.extend(std::iter::repeat_n('\\', backslashes));
                    }
                }
                '"' => {
                    quoted = !quoted;
                    chars.next();
                }
                _ => {
                    arg.push(c);
                    chars.next();
                }
            }
        }

        args.push(arg);
    }

    args
}

/// A `;` delimited list. Trimmed, empties dropped, order preserved.
fn list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
}

/// An MSBuildboolean, which is `true`/`false` case-insensitively.
///
/// Anything else emits nothing rather than defaulting to either flag.
fn boolean(flags: &mut Vec<String>, value: &str, when_true: &str, when_false: &str) {
    let flag = if value.eq_ignore_ascii_case("true") {
        when_true
    } else if value.eq_ignore_ascii_case("false") {
        when_false
    } else {
        return;
    };

    if !flag.is_empty() {
        flags.push(flag.to_string());
    }
}

/// Keep the first occurrence of each entry.
///
/// `cl` searches include directories in the order given, so first-wins is the only order that
/// preserves search semantics. For defines, a later `/D` of the same macro wins, and dropping an
/// exact duplicate cannot change that.
fn dedup<T: PartialEq>(entries: Vec<T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(entries.len());

    for entry in entries {
        if !out.contains(&entry) {
            out.push(entry);
        }
    }

    out
}

// Deliberately not translated:
//
//   Optimization, InlineFunctionExpansion, IntrinsicFunctions, FunctionLevelLinking,
//   WholeProgramOptimization, StringPooling, BufferSecurityCheck, BasicRuntimeChecks, SDLCheck,
//   ControlFlowGuard, DebugInformationFormat, MinimalRebuild, MultiProcessorCompilation,
//   ObjectFileName, ProgramDataBaseFileName, XMLDocumentationFileName
//       Codegen or build bookkeeping. None of them can change how a source file parses.
//
//   TreatWarningAsError (`/WX`)
//       clang's warning set is not MSVC's, so honouring it turns diagnostics the real build never
//       emits into editor errors and makes a working file read as broken.
//
//   PrecompiledHeader, PrecompiledHeaderFile (`/Yu`, `/Yc`)
//       `/Yu` injects nothing — a source using a precompiled header still `#include`s it — so
//       skipping costs no parse fidelity, while emitting it points clangd at a `.pch` that no
//       sln2cc run produces.
//
//   DisableLanguageExtensions (`/Za`)
//       clang-cl rejects it, which would fail the whole command rather than one flag.
//
//   CharacterSet, ConfigurationType (properties, not metadata)
//       `UNICODE`/`_UNICODE` and `_WINDLL` reach `PreprocessorDefinitions` through Microsoft's own
//       `.props`/`.targets`, which are not imported until MVP-15 supplies `$(VCTargetsPath)`.
//       Special-casing them here would hard-code a copy of Microsoft's logic that then goes stale.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reporter;
    use crate::msbuild::eval::Metadata;
    use crate::msbuild::items::SourceItem;
    use std::path::PathBuf;

    fn absolute(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    /// The project directory every case resolves against.
    fn project_dir() -> PathBuf {
        absolute(&["code", "app"])
    }

    /// A settled item carrying `metadata`, as `items::settle` would produce it.
    fn source(metadata: &[(&str, &str)]) -> SourceItem {
        let mut bag = Metadata::for_item("ClCompile");
        for (name, value) in metadata {
            bag.set(name, *value);
        }

        SourceItem {
            path: absolute(&["code", "app", "main.cpp"]),
            metadata: bag,
            declared_in: absolute(&["code", "app", "App.vcxproj"]),
            line: 11,
        }
    }

    /// Compile `metadata`, asserting nothing was warned about — what most cases want.
    fn compiled(metadata: &[(&str, &str)]) -> FileCompilation {
        let mut reporter = Reporter::new(false);
        let compilation = compile(&source(metadata), &project_dir(), &mut reporter);
        assert!(
            reporter.warnings().is_empty(),
            "unexpected warnings: {:?}",
            reporter.warnings()
        );
        compilation
    }

    /// Compile and hand back the reporter, for the cases asserting on what was warned about.
    fn compiled_reporting(metadata: &[(&str, &str)]) -> (FileCompilation, Reporter) {
        let mut reporter = Reporter::new(false);
        let compilation = compile(&source(metadata), &project_dir(), &mut reporter);
        (compilation, reporter)
    }

    /// Just the flags, which is what most of the table cases assert on.
    fn flags_of(metadata: &[(&str, &str)]) -> Vec<String> {
        compiled(metadata).flags
    }

    #[test]
    fn the_file_comes_through_as_the_item_settled_it() {
        // `compile` never re-resolves the path: `items::settle` already made it absolute, and doing
        // it twice would give two chances to get it wrong.
        let compilation = compiled(&[]);

        assert_eq!(compilation.file, absolute(&["code", "app", "main.cpp"]));
    }

    #[test]
    fn an_item_with_no_metadata_produces_no_flags() {
        // Reachable: a `.vcxproj` with no `ItemDefinitionGroup` at all, which is what a project
        // looks like before Microsoft's own `.props` are imported. Whatever comes out has to be
        // empty rather than a set of invented defaults.
        let compilation = compiled(&[]);

        assert!(compilation.include_paths.is_empty());
        assert!(compilation.defines.is_empty());
        assert!(compilation.flags.is_empty());
    }

    #[test]
    fn include_directories_become_absolute_paths_in_order() {
        // Search order is semantic: `cl` reads the directories in the order given, so a header
        // present in two of them resolves to the first. Sorting these would silently change which.
        let compilation = compiled(&[(
            "AdditionalIncludeDirectories",
            r"inc; ..\shared\inc ;vendor\zlib",
        )]);

        assert_eq!(
            compilation.include_paths,
            [
                absolute(&["code", "app", "inc"]),
                absolute(&["code", "shared", "inc"]),
                absolute(&["code", "app", "vendor", "zlib"]),
            ],
            "entries are trimmed and order is kept"
        );
    }

    #[test]
    fn an_empty_include_entry_is_dropped() {
        // `%(AdditionalIncludeDirectories)` expanding to nothing leaves `inc;;` or a trailing `;`
        // in every real project file. An empty entry would resolve to the project directory itself.
        let compilation = compiled(&[("AdditionalIncludeDirectories", "inc;;")]);

        assert_eq!(
            compilation.include_paths,
            [absolute(&["code", "app", "inc"])]
        );
    }

    #[test]
    fn a_repeated_include_directory_is_kept_once() {
        // Two property sheets contributing the same directory is ordinary, and a duplicate `/I`
        // changes nothing except how long clangd takes to say so.
        let compilation = compiled(&[("AdditionalIncludeDirectories", r"inc;.\inc;inc\")]);

        assert_eq!(
            compilation.include_paths,
            [absolute(&["code", "app", "inc"])],
            "three spellings of one directory come out once"
        );
    }

    #[test]
    fn defines_are_split_and_trimmed_but_not_otherwise_touched() {
        // A define may carry a value (`VERSION=2`) or a string (`NAME=\"app\"`). Neither is parsed
        // here — the whole entry is what `/D` takes.
        let compilation = compiled(&[("PreprocessorDefinitions", "WIN32; _DEBUG ;VERSION=2;;")]);

        assert_eq!(compilation.defines, ["WIN32", "_DEBUG", "VERSION=2"]);
    }

    #[test]
    fn a_repeated_define_is_kept_once() {
        let compilation = compiled(&[("PreprocessorDefinitions", "WIN32;NDEBUG;WIN32")]);

        assert_eq!(compilation.defines, ["WIN32", "NDEBUG"]);
    }

    #[test]
    fn every_enumerated_setting_maps_to_its_cl_switch() {
        // One case per mapping, which is the table this issue exists to write. A wrong entry here
        // is a flag clangd acts on and the real build does not.
        let cases = [
            ("CompileAs", "CompileAsC", "/TC"),
            ("CompileAs", "CompileAsCpp", "/TP"),
            ("LanguageStandard", "stdcpp14", "/std:c++14"),
            ("LanguageStandard", "stdcpp17", "/std:c++17"),
            ("LanguageStandard", "stdcpp20", "/std:c++20"),
            ("LanguageStandard", "stdcpplatest", "/std:c++latest"),
            ("LanguageStandard_C", "stdc11", "/std:c11"),
            ("LanguageStandard_C", "stdc17", "/std:c17"),
            ("RuntimeLibrary", "MultiThreaded", "/MT"),
            ("RuntimeLibrary", "MultiThreadedDebug", "/MTd"),
            ("RuntimeLibrary", "MultiThreadedDLL", "/MD"),
            ("RuntimeLibrary", "MultiThreadedDebugDLL", "/MDd"),
            ("ExceptionHandling", "Sync", "/EHsc"),
            ("ExceptionHandling", "Async", "/EHa"),
            ("ExceptionHandling", "SyncCThrow", "/EHs"),
            (
                "EnableEnhancedInstructionSet",
                "StreamingSIMDExtensions",
                "/arch:SSE",
            ),
            (
                "EnableEnhancedInstructionSet",
                "StreamingSIMDExtensions2",
                "/arch:SSE2",
            ),
            (
                "EnableEnhancedInstructionSet",
                "AdvancedVectorExtensions",
                "/arch:AVX",
            ),
            (
                "EnableEnhancedInstructionSet",
                "AdvancedVectorExtensions2",
                "/arch:AVX2",
            ),
            (
                "EnableEnhancedInstructionSet",
                "AdvancedVectorExtensions512",
                "/arch:AVX512",
            ),
            ("EnableEnhancedInstructionSet", "NoExtensions", "/arch:IA32"),
            ("StructMemberAlignment", "1Byte", "/Zp1"),
            ("StructMemberAlignment", "16Bytes", "/Zp16"),
            ("CallingConvention", "Cdecl", "/Gd"),
            ("CallingConvention", "FastCall", "/Gr"),
            ("CallingConvention", "StdCall", "/Gz"),
            ("CallingConvention", "VectorCall", "/Gv"),
            ("FloatingPointModel", "Precise", "/fp:precise"),
            ("FloatingPointModel", "Strict", "/fp:strict"),
            ("FloatingPointModel", "Fast", "/fp:fast"),
            ("WarningLevel", "TurnOffAllWarnings", "/W0"),
            ("WarningLevel", "Level3", "/W3"),
            ("WarningLevel", "Level4", "/W4"),
            ("WarningLevel", "EnableAllWarnings", "/Wall"),
        ];

        for (name, value, expected) in cases {
            assert_eq!(flags_of(&[(name, value)]), [expected], "{name}={value}");
        }
    }

    #[test]
    fn a_boolean_setting_maps_both_ways() {
        // `/GR-` is not the absence of `/GR`: it turns RTTI *off*, and a file compiled without it
        // has `_CPPRTTI` defined. Both spellings have to reach clangd.
        let cases = [
            ("ConformanceMode", "true", "/permissive-"),
            ("ConformanceMode", "false", "/permissive"),
            ("RuntimeTypeInfo", "true", "/GR"),
            ("RuntimeTypeInfo", "false", "/GR-"),
            ("TreatWChar_tAsBuiltInType", "true", "/Zc:wchar_t"),
            ("TreatWChar_tAsBuiltInType", "false", "/Zc:wchar_t-"),
            ("OpenMPSupport", "true", "/openmp"),
        ];

        for (name, value, expected) in cases {
            assert_eq!(flags_of(&[(name, value)]), [expected], "{name}={value}");
        }
    }

    #[test]
    fn a_boolean_is_matched_case_insensitively() {
        // MSBuild booleans are `true`/`false` however they are spelled, and `True` is what some
        // hand-written sheets carry.
        assert_eq!(flags_of(&[("RuntimeTypeInfo", "True")]), ["/GR"]);
        assert_eq!(flags_of(&[("RuntimeTypeInfo", "FALSE")]), ["/GR-"]);
    }

    #[test]
    fn a_boolean_with_no_off_switch_emits_nothing_when_false() {
        // There is no "not OpenMP" flag, and inventing one would be worse than omitting it.
        assert!(flags_of(&[("OpenMPSupport", "false")]).is_empty());
    }

    #[test]
    fn an_enumerated_value_is_matched_case_insensitively() {
        assert_eq!(
            flags_of(&[("LanguageStandard", "STDCPP20")]),
            ["/std:c++20"]
        );
    }

    #[test]
    fn exception_handling_false_is_a_real_value_that_maps_to_no_flag() {
        // `<ExceptionHandling>false</ExceptionHandling>` is what Visual Studio writes when
        // exceptions are off. It is a recognized value, so it must not warn — but there is no `cl`
        // switch for it either, since off is the default.
        let (compilation, reporter) = compiled_reporting(&[("ExceptionHandling", "false")]);

        assert!(compilation.flags.is_empty());
        assert!(
            reporter.warnings().is_empty(),
            "a recognized value must not warn: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn a_default_or_empty_value_emits_nothing() {
        // `Default` means "whatever cl does", which is also whatever clang-cl does. Emitting a
        // guess here would pin down a setting the project deliberately left open.
        for value in ["Default", "", "  "] {
            let (compilation, reporter) = compiled_reporting(&[
                ("LanguageStandard", value),
                ("RuntimeLibrary", value),
                ("CompileAs", value),
            ]);

            assert!(compilation.flags.is_empty(), "for {value:?}");
            assert!(
                reporter.warnings().is_empty(),
                "an unset setting is not worth a warning: {:?}",
                reporter.warnings()
            );
        }
    }

    #[test]
    fn an_unrecognized_value_warns_and_is_dropped() {
        // A newer toolset's value, or a typo. Guessing would produce a flag the build never used;
        // silence would leave a user wondering why `/std:` never appears. CLAUDE.md §3.
        let (compilation, reporter) = compiled_reporting(&[("LanguageStandard", "stdcpp26")]);

        assert!(compilation.flags.is_empty());

        let [warning] = reporter.warnings() else {
            panic!("expected exactly one warning: {:?}", reporter.warnings());
        };
        assert!(
            warning.message.contains("LanguageStandard") && warning.message.contains("stdcpp26"),
            "names the setting and the value: {}",
            warning.message
        );
        assert_eq!(warning.line, Some(11), "the item's line");
        assert_eq!(
            warning.file.as_deref(),
            Some(absolute(&["code", "app", "App.vcxproj"]).as_path()),
            "the file that declared the item"
        );
    }

    #[test]
    fn undefines_become_switches_in_order() {
        assert_eq!(
            flags_of(&[("UndefinePreprocessorDefinitions", "_MSC_VER;WIN32")]),
            ["/U_MSC_VER", "/UWIN32"]
        );
        assert_eq!(
            flags_of(&[("UndefineAllPreprocessorDefinitions", "true")]),
            ["/u"]
        );
    }

    #[test]
    fn a_forced_include_is_made_absolute() {
        // `/FI` is resolved by the compiler against its own working directory, and clangd's is not
        // the project's. A relative one here is a header clangd reports as missing.
        let flags = flags_of(&[("ForcedIncludeFiles", r"inc\prefix.h")]);

        assert_eq!(
            flags,
            [format!(
                "/FI{}",
                absolute(&["code", "app", "inc", "prefix.h"]).display()
            )]
        );
    }

    #[test]
    fn disabled_warnings_become_switches() {
        assert_eq!(
            flags_of(&[("DisableSpecificWarnings", "4996;4244")]),
            ["/wd4996", "/wd4244"]
        );
    }

    #[test]
    fn additional_options_come_through_verbatim_and_last() {
        // Last so a hand-written switch wins, which is the order `cl` itself resolves conflicts in:
        // a project that sets `LanguageStandard` and then overrides it in `AdditionalOptions` means
        // the override.
        let flags = flags_of(&[
            ("LanguageStandard", "stdcpp17"),
            ("AdditionalOptions", "/std:c++20 /Zc:__cplusplus"),
        ]);

        assert_eq!(flags, ["/std:c++17", "/std:c++20", "/Zc:__cplusplus"]);
    }

    /// MVP-38 replacing MVP-13's warning with the real fix: a quoted argument containing a space
    /// now survives as the one flag the project wrote, instead of splitting apart. See
    /// [ADR 43](../../../docs/adr/43-additional-options-argument-splitting.md).
    #[test]
    fn additional_options_splits_a_quoted_argument_with_a_space() {
        let (compilation, reporter) =
            compiled_reporting(&[("AdditionalOptions", r#"/DBANNER="hello world""#)]);

        assert_eq!(
            compilation.flags,
            [r#"/DBANNER=hello world"#],
            "the quoted argument reaches clangd as one flag, not two"
        );
        assert!(
            reporter.warnings().is_empty(),
            "the split is correct now, nothing to warn about: {:?}",
            reporter.warnings()
        );
    }

    /// The shape named in the todo: a quoted, space-containing path handed to `/I`. Getting this
    /// wrong points clangd at two directories that do not exist.
    #[test]
    fn additional_options_splits_a_quoted_path_in_an_include_switch() {
        let flags = flags_of(&[("AdditionalOptions", r#"/I"C:\Program Files\x\include""#)]);

        assert_eq!(flags, [r#"/IC:\Program Files\x\include"#]);
    }

    /// The negative case, so the check cannot be satisfied by always splitting on quotes. Nearly
    /// every real `AdditionalOptions` looks like this one, and it must split exactly as the old
    /// whitespace-only implementation did.
    #[test]
    fn additional_options_without_quotes_splits_on_whitespace_as_before() {
        let flags = flags_of(&[("AdditionalOptions", "/Zc:__cplusplus /bigobj")]);

        assert_eq!(flags, ["/Zc:__cplusplus", "/bigobj"]);
    }

    /// `.vcxproj` input is untrusted (CLAUDE.md §5): a quote opened but never closed must not hang
    /// evaluation or silently drop everything after it.
    #[test]
    fn additional_options_with_an_unterminated_quote_does_not_drop_the_rest() {
        let flags = flags_of(&[("AdditionalOptions", r#"/DFOO="bar baz"#)]);

        assert_eq!(flags, [r#"/DFOO=bar baz"#]);
    }

    /// The odd-backslash-count case: `\"` inside a quoted region is a literal `"`, not the end of
    /// the quote, matching how `cl.exe`/clang-cl parse their own argument list.
    #[test]
    fn additional_options_with_an_embedded_escaped_quote() {
        let flags = flags_of(&[("AdditionalOptions", r#"/DMSG="say \"hi\"""#)]);

        assert_eq!(flags, [r#"/DMSG=say "hi""#]);
    }

    /// An absent `AdditionalOptions` is the common case for most sources and must not manufacture
    /// an empty flag.
    #[test]
    fn additional_options_with_an_empty_value_produces_no_flags() {
        let flags = flags_of(&[("AdditionalOptions", "")]);

        assert!(flags.is_empty(), "unexpected flags: {flags:?}");
    }

    /// Direct coverage of the splitter's backslash counting, isolated from `compile` since these
    /// two shapes are about the algorithm rather than any particular MSBuild setting.
    #[test]
    fn split_arguments_copies_backslashes_not_followed_by_a_quote_literally() {
        assert_eq!(
            split_arguments(r"\path\to\thing"),
            [r"\path\to\thing"],
            "no quote anywhere, so nothing is special about the backslashes"
        );
    }

    /// An even run of backslashes before a quote halves into literal backslashes and the quote
    /// still toggles quoting state — the complement of the odd-run case covered through `compile`.
    #[test]
    fn split_arguments_halves_an_even_backslash_run_before_a_quote() {
        assert_eq!(split_arguments(r#"\\\\"x""#), [r"\\x"]);
    }

    #[test]
    fn a_codegen_or_link_setting_produces_nothing() {
        // The skip list, asserted rather than described. None of these can change how a file
        // parses, and each one emitted would be a flag clangd has to have an opinion about.
        let (compilation, reporter) = compiled_reporting(&[
            ("Optimization", "MaxSpeed"),
            ("InlineFunctionExpansion", "AnySuitable"),
            ("IntrinsicFunctions", "true"),
            ("FunctionLevelLinking", "true"),
            ("WholeProgramOptimization", "true"),
            ("StringPooling", "true"),
            ("BufferSecurityCheck", "true"),
            ("BasicRuntimeChecks", "EnableFastChecks"),
            ("SDLCheck", "true"),
            ("ControlFlowGuard", "Guard"),
            ("DebugInformationFormat", "ProgramDatabase"),
            ("MinimalRebuild", "false"),
            ("MultiProcessorCompilation", "true"),
            ("ObjectFileName", r"$(IntDir)"),
            ("ProgramDataBaseFileName", r"$(IntDir)vc143.pdb"),
        ]);

        assert!(
            compilation.flags.is_empty(),
            "unexpected flags: {:?}",
            compilation.flags
        );
        assert!(
            reporter.warnings().is_empty(),
            "an unread setting is not a problem and must not warn: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn the_deliberately_unsupported_settings_produce_nothing() {
        // Each of these is skipped for a reason of its own — recorded at the bottom of this module
        // and in ADR 12 — rather than because nobody got to it. They must not warn either: the
        // project is correct, and this tool's choice not to act on it is not the user's problem.
        let (compilation, reporter) = compiled_reporting(&[
            ("TreatWarningAsError", "true"),
            ("PrecompiledHeader", "Use"),
            ("PrecompiledHeaderFile", "pch.h"),
            ("DisableLanguageExtensions", "true"),
        ]);

        assert!(
            compilation.flags.is_empty(),
            "unexpected flags: {:?}",
            compilation.flags
        );
        assert!(
            reporter.warnings().is_empty(),
            "unexpected warnings: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn an_unresolved_reference_in_a_path_or_define_warns() {
        // Pass 1 and pass 2 copy a construct they cannot evaluate through verbatim so it names its
        // own cause. This is where that becomes a sentence: `$(VCTargetsPath)` is undefined until
        // MVP-15, and an include path built from it is garbage rather than merely odd.
        let (_, reporter) = compiled_reporting(&[
            ("AdditionalIncludeDirectories", "$(VCTargetsPath)inc"),
            ("PreprocessorDefinitions", "%(ClCompile.Identity)"),
        ]);

        assert_eq!(reporter.warnings().len(), 2, "{:?}", reporter.warnings());
        assert!(
            reporter.warnings()[0]
                .message
                .contains("AdditionalIncludeDirectories"),
            "names the setting: {}",
            reporter.warnings()[0].message
        );
        assert!(
            reporter.warnings()[1]
                .message
                .contains("PreprocessorDefinitions"),
            "names the setting: {}",
            reporter.warnings()[1].message
        );
    }

    #[test]
    fn a_resolved_value_does_not_warn() {
        // The guard above must not fire on a define that merely contains a dollar or a percent —
        // `NAME="100%"` is legal and `$` appears in generated headers' guards.
        let (_, reporter) = compiled_reporting(&[
            ("PreprocessorDefinitions", r#"RATE=100%;NAME="a$b""#),
            ("AdditionalIncludeDirectories", "inc"),
        ]);

        assert!(
            reporter.warnings().is_empty(),
            "unexpected warnings: {:?}",
            reporter.warnings()
        );
    }

    #[test]
    fn flag_order_is_stable_across_runs() {
        // Golden files and `git diff` both depend on this, and a `HashMap` iteration order leaking
        // into the output is the classic way to lose it — `Metadata` is a `HashMap` inside.
        let metadata = [
            ("CompileAs", "CompileAsCpp"),
            ("LanguageStandard", "stdcpp20"),
            ("RuntimeLibrary", "MultiThreadedDebugDLL"),
            ("ExceptionHandling", "Sync"),
            ("ConformanceMode", "true"),
            ("RuntimeTypeInfo", "true"),
            ("DisableSpecificWarnings", "4996"),
            ("AdditionalOptions", "/Zc:__cplusplus"),
        ];

        let first = flags_of(&metadata);
        for _ in 0..8 {
            assert_eq!(flags_of(&metadata), first);
        }

        assert_eq!(
            first.last().map(String::as_str),
            Some("/Zc:__cplusplus"),
            "AdditionalOptions stays last however the bag iterates"
        );
    }
}
