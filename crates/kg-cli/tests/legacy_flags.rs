//! The compatibility scoreboard: every flag in `docs/spec/cli.md`'s inventory,
//! exercised through the public parser.
//!
//! The inventory is data (`kg_cli::legacy::flags::FLAGS`), so this is one test
//! over 128 spellings rather than 128 tests. What it proves per flag:
//!
//! * the long form parses, in every mode that declares it;
//! * the value actually lands in the option record — a flag that parsed and
//!   then dropped its value would pass a "does it parse" test and silently
//!   change every compilation;
//! * the space-separated form parses wherever the reference accepts it;
//! * the declared short alias parses.

use kg_cli::legacy::flags::{Builtin, Def, FLAGS, FlagSpec, Group, Sink};
use kg_cli::legacy::options::{LegacyOptions, Mode};
use kg_cli::legacy::parse::{Outcome, parse};

fn argv(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn parse_ok(args: &[String]) -> LegacyOptions {
    match parse(args) {
        Outcome::Parsed(options) => *options,
        other => panic!("{args:?} should parse, got {other:?}"),
    }
}

fn parsed(list: &[&str]) -> LegacyOptions {
    parse_ok(&argv(list))
}

/// A value that is guaranteed to differ from the flag's default, so "the option
/// record changed" is a sound proxy for "the value was stored".
fn sample(flag: &FlagSpec) -> Option<String> {
    match flag.default {
        Def::Int(n) => Some(n.wrapping_add(1).to_string()),
        Def::MaxInt => Some("0".to_owned()),
        Def::Float(x) => Some(format!("{}", x + 1.5)),
        Def::Text(t) => Some(format!("{t}.sample")),
        Def::List => Some("sample-value".to_owned()),
        Def::Unset => Some("4".to_owned()),
        Def::Enum("none") => Some("sv".to_owned()),
        Def::Enum("pretty") => Some("smart".to_owned()),
        Def::Enum(other) => panic!("no sample for enum default {other}"),
        // Switches and warning switches carry no value.
        Def::Bool(_) | Def::None => None,
    }
}

fn baseline(mode: Mode) -> LegacyOptions {
    LegacyOptions { mode, files: vec!["a.k".to_owned()], ..LegacyOptions::default() }
}

/// The argv prefix that selects a mode. `compile` is the default and may be
/// omitted, so the compile case also proves the omission works.
fn mode_prefix(mode: Mode) -> Vec<String> {
    match mode {
        Mode::Compile => Vec::new(),
        Mode::Pretty => vec!["pretty".to_owned()],
        Mode::ListDeps => vec!["list-deps".to_owned()],
    }
}

/// `<mode?> <flag…> a.k`
fn line(mode: Mode, flag: &[&str]) -> Vec<String> {
    let mut args = mode_prefix(mode);
    args.extend(flag.iter().map(|s| (*s).to_owned()));
    args.push("a.k".to_owned());
    args
}

#[test]
fn every_flag_in_the_inventory_parses_and_records_its_value() {
    let mut checked = 0usize;

    for flag in FLAGS {
        for mode in Mode::ALL {
            if !flag.modes.accepts(mode) {
                continue;
            }
            // The three built-ins that short-circuit parsing are covered by
            // `the_built_ins_short_circuit` below; they are counted once, not
            // once per mode, exactly as cli.md §1.0 counts them.
            if matches!(
                flag.sink,
                Sink::Builtin(Builtin::Help | Builtin::Version | Builtin::NumericVersion)
            ) {
                checked += usize::from(mode == Mode::Compile);
                continue;
            }
            if matches!(flag.sink, Sink::Builtin(_)) {
                checked += usize::from(mode == Mode::Compile);
            } else {
                checked += 1;
            }

            let value = sample(flag);
            let attached = match &value {
                Some(value) => format!("--{}={value}", flag.long),
                None => format!("--{}", flag.long),
            };

            let options = parse_ok(&line(mode, &[&attached]));
            assert_eq!(options.files, ["a.k"], "{attached}: the source file was consumed");
            assert_eq!(options.mode, mode, "{attached}: the mode changed");
            assert_ne!(
                options,
                baseline(mode),
                "--{} parsed but left the option record untouched",
                flag.long
            );

            // The space-separated form, where the reference accepts one.
            if let (true, Some(value)) = (flag.takes_separate_value(), &value) {
                let separated = parse_ok(&line(mode, &[&format!("--{}", flag.long), value]));
                assert_eq!(
                    separated, options,
                    "--{0} <value> should mean the same as --{0}=<value>",
                    flag.long
                );
            }

            // Every declared long alias, and the declared short one.
            for alias in flag.aliases {
                let spelling = match &value {
                    Some(value) => format!("--{alias}={value}"),
                    None => format!("--{alias}"),
                };
                assert_eq!(
                    parse_ok(&line(mode, &[&spelling])),
                    options,
                    "--{alias} should mean --{}",
                    flag.long
                );
            }
            // A short alias is per mode: `cmdargs` assigns them mode by mode
            // (`-b` is `--base-library` in `pretty` only), so outside its
            // modes the letter is refused, not taken for another flag.
            if let Some(short) = flag.short {
                let spelling = match &value {
                    Some(value) => format!("-{short}{value}"),
                    None => format!("-{short}"),
                };
                if flag.short_modes.accepts(mode) {
                    assert_eq!(
                        parse_ok(&line(mode, &[&spelling])),
                        options,
                        "-{short} should mean --{}",
                        flag.long
                    );
                } else {
                    assert!(
                        matches!(parse(&line(mode, &[&spelling])), Outcome::Failed(_)),
                        "-{short} is not --{} in {mode:?}",
                        flag.long
                    );
                }
            }
        }
    }

    // cli.md §1.0: 103 compile + 12 pretty + 5 built-ins; and `list-deps`'
    // 8, which postdate it (`Options.hs:133-141`).
    assert_eq!(checked, 128, "the inventory should cover 128 flag spellings");
    assert_eq!(kg_cli::legacy::flags::spelling_count(), 128);
}

#[test]
fn the_built_ins_short_circuit() {
    assert!(matches!(parse(&argv(&["--help"])), Outcome::Help(_)));
    assert!(matches!(parse(&argv(&["pretty", "--help"])), Outcome::Help(_)));
    assert!(matches!(parse(&argv(&["--version"])), Outcome::Version(_)));
    assert!(matches!(parse(&argv(&["--numeric-version"])), Outcome::NumericVersion(_)));
    // Verbosity is recorded rather than short-circuiting.
    assert_ne!(parsed(&["--verbose", "a.k"]), baseline(Mode::Compile));
    assert_ne!(parsed(&["--quiet", "a.k"]), baseline(Mode::Compile));
}

#[test]
fn the_inventory_covers_the_flags_the_reference_test_suite_actually_uses() {
    // docs/spec/cli.md §8.10: the 33 distinct flags exercised by the whole
    // upstream test suite. If any of these regress, real CI breaks.
    let used = [
        "Wall", "Werror", "Wconversion", "Wredundant-import", "Wno-initializer-overrides",
        "Wno-redundant-import", "Wno-shadow", "backend", "base-library",
        "check-narrowing-assignment", "debug", "define", "force-mem-to-array", "frequency",
        "import-dir", "jobs", "max-block-ram", "mem-to-reg", "no-implicit-base", "optimize",
        "output", "parse-docs", "place-iterations", "register-ratio", "reset-cycles",
        "rmw-hardened-write-delay", "rmw-memory-read-delay", "rmw-memory-write-delay",
        "skip-circt-lowering", "stall", "target-device", "template-iterations", "using",
    ];
    assert_eq!(used.len(), 33);
    for name in used {
        assert!(
            FLAGS.iter().any(|f| f.long_names().any(|n| n == name)),
            "the suite uses --{name} and the inventory does not have it"
        );
    }
}

#[test]
fn every_flag_declares_the_group_the_reference_puts_it_in() {
    // `--help` sectioning is the only thing `&= groupname` affects, but a wrong
    // group is a sign the row was transcribed from the wrong line.
    let count = |group: Group| FLAGS.iter().filter(|f| f.group == group).count();
    assert_eq!(count(Group::Logic), 40);
    assert_eq!(count(Group::Compiler), 7);
    assert_eq!(count(Group::Output), 7);
    assert_eq!(count(Group::CodeGeneration), 15);
    assert_eq!(count(Group::Placement), 7);
    assert_eq!(count(Group::Diagnostics), 13);
    assert_eq!(count(Group::Warnings), 14);
    // `pretty`'s four declare none, and take the field's before them.
    assert_eq!(count(Group::Undeclared), 4);
    assert_eq!(count(Group::Common), 5);
}

#[test]
fn defines_and_usings_split_on_hash_first() {
    // `--define=ELEVEN#11` (test/syntax/CMakeLists.txt:82) and
    // `--define=MemoryLength=32` (tools/chkdoc/chkdoc.cpp:41) must both work.
    let options = parsed(&[
        "--define=ELEVEN#11",
        "--define=MemoryLength=32",
        "--using=COMPILE_PARAM_1#uint8",
        "--using=T=uint16",
        "a.k",
    ]);
    assert_eq!(options.defines(), [("ELEVEN", "11"), ("MemoryLength", "32")]);
    assert_eq!(options.usings(), [("COMPILE_PARAM_1", "uint8"), ("T", "uint16")]);
    assert_eq!(
        options.cmdargs_module_source(),
        "module .cmdargs{module .cmdargs}\n\
         const auto ELEVEN = 11;\n\
         const auto MemoryLength = 32;\n\
         using COMPILE_PARAM_1 = uint8;\n\
         using T = uint16;\n"
    );
}

#[test]
fn repeated_import_dirs_keep_their_order_and_expand_devices_first() {
    let options = parsed(&[
        "--import-dir=/root/library",
        "--import-dir",
        "/root/test/logic",
        "--import-dir=/root/vendor",
        "--target-device=stratix-10-gx-2800",
        "a.k",
    ]);
    assert_eq!(options.import_dir, ["/root/library", "/root/test/logic", "/root/vendor"]);
    assert_eq!(
        options.import_search_path("/src"),
        [
            "/src",
            "/root/library/device/stratix-10-gx-2800",
            "/root/test/logic/device/stratix-10-gx-2800",
            "/root/vendor/device/stratix-10-gx-2800",
            "/root/library",
            "/root/test/logic",
            "/root/vendor",
        ]
    );
}

#[test]
fn warning_switches_accumulate_rather_than_replace() {
    use kg_diag::WarningKind::*;

    // The whole point, proved by test/syntax/shadow-warning.k: one --Wno- switch
    // must not wipe out the other three defaults.
    let only_one_disabled = parsed(&["--Wno-redundant-import", "a.k"]).enabled_warnings();
    assert!(only_one_disabled.is_enabled(Shadow));
    assert!(only_one_disabled.is_enabled(Deprecated));
    assert!(only_one_disabled.is_enabled(InitializerOverrides));
    assert!(!only_one_disabled.is_enabled(RedundantImport));

    // The logic suite's own set: --Wall --Werror … --Wno-initializer-overrides --Wno-shadow.
    let logic = parsed(&[
        "--Wall",
        "--Werror",
        "--Wno-initializer-overrides",
        "--Wno-shadow",
        "a.k",
    ]);
    let set = logic.enabled_warnings();
    assert!(logic.warnings_as_errors);
    assert_eq!(
        [Conversion, Deprecated, RedundantImport, TransactionSize]
            .map(|kind| set.is_enabled(kind)),
        [true; 4]
    );
    assert_eq!([InitializerOverrides, Shadow].map(|kind| set.is_enabled(kind)), [false; 2]);

    // Order matters, and the last switch wins.
    assert!(!parsed(&["--Wall", "--Wno-conversion", "a.k"]).enabled_warnings().is_enabled(Conversion));
    assert!(parsed(&["--Wno-conversion", "--Wall", "a.k"]).enabled_warnings().is_enabled(Conversion));

    // --Wall adds exactly the two kinds that are off by default; it does not
    // re-enable one an earlier switch turned off.
    assert!(!parsed(&["--Wno-shadow", "--Wall", "a.k"]).enabled_warnings().is_enabled(Shadow));
}

#[test]
fn the_debugging_axis_survives_verbatim() {
    // --frontend-passes / --template-passes / --template-iterations are the
    // project's binary-search debugging tool; the values must be preserved
    // exactly, including the bare "all" form.
    let bare = parsed(&["--frontend-passes", "--template-passes", "--template-iterations", "a.k"]);
    assert_eq!(bare.passes, i64::MAX);
    assert_eq!(bare.template_passes, i64::MAX);
    assert_eq!(bare.template_iterations, i64::MAX);
    assert_eq!(bare.files, ["a.k"], "a bare opt-valued flag must not eat the file");

    let explicit =
        parsed(&["--frontend-passes=7", "--template-passes=0", "--template-iterations=1", "a.k"]);
    assert_eq!(explicit.passes, 7);
    assert_eq!(explicit.template_passes, 0);
    assert_eq!(explicit.template_iterations, 1);
}

#[test]
fn cmd_args_is_reconstructed_the_way_the_rtl_header_needs_it() {
    use kg_cli::legacy::cmd_args_string;

    let build = |args: &[&str]| cmd_args_string(&argv(args));

    // `unwords (basename argv[0] : argv[1..])`, no quoting, no escaping.
    assert_eq!(
        build(&["/usr/local/bin/kanagawa", "--backend=sv", "source.k"]),
        "kanagawa --backend=sv source.k"
    );
    assert_eq!(build(&["./kanagawa", "compile", "a.k"]), "kanagawa compile a.k");
    assert_eq!(build(&["/opt/kanagawa-1.2.3", "a.k"]), "kanagawa-1.2.3 a.k");
    assert_eq!(build(&["kanagawa.exe", "a.k"]), "kanagawa.exe a.k");
    assert_eq!(build(&["kanagawa", "--import-dir=/My Libs", "a.k"]), "kanagawa --import-dir=/My Libs a.k");
    assert_eq!(build(&["kanagawa"]), "kanagawa");

    // The full STALL2 logic-suite command line, which is what a byte-for-byte
    // RTL comparison would be reading out of the generated `.sv` header.
    let stall2 = build(&[
        "/build/bin/kanagawa",
        "--Wall",
        "--Werror",
        "--backend=sv",
        "--define=verbose#true",
        "--debug",
        "--stall=2",
        "--output",
        "/build/test/logic/logic.STALL2.stall/",
        "stall.k",
    ]);
    assert_eq!(
        stall2,
        "kanagawa --Wall --Werror --backend=sv --define=verbose#true --debug --stall=2 --output /build/test/logic/logic.STALL2.stall/ stall.k"
    );
}
