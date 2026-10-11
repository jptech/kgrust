//! The argv parser for the `kanagawa` compat binary.
//!
//! Hand-rolled rather than `clap`-derived, because the compatibility target is
//! the Haskell `cmdargs` library and four of its behaviours have no `clap`
//! equivalent:
//!
//! 1. `--flag=false` on a switch (`test/syntax/CMakeLists.txt:76`);
//! 2. unambiguous long-flag **prefix** matching — `--base` resolves to
//!    `--base-library` (`tools/chkdoc/chkdoc.cpp:208`);
//! 3. optional-value flags (`&= opt`) that mean `maxBound` when bare;
//! 4. `warnings_state`, where each switch *appends* to a list that starts with
//!    four defaults, and the list is folded left at the end.
//!
//! Everything the reference leaves UNVERIFIED (`docs/spec/cli.md` §11) is
//! resolved here in the direction that keeps every in-tree invocation working,
//! and the choice is commented at the point it is made.

use super::flags::{Builtin, FLAGS, FlagSpec, Sink};
use super::help::{self, Scope};
use super::options::{Backend, Layout, LegacyOptions, Mode, Verbosity};

/// What the command line asked for.
#[derive(Debug)]
pub enum Outcome {
    /// Print to stdout, exit 0.
    Help(String),
    /// Print to stdout, exit 0.
    Version(String),
    /// Print to stdout, exit 0.
    NumericVersion(String),
    /// Run the compiler.
    Parsed(Box<LegacyOptions>),
    /// `cmdargs` writes its own parse failures to **stderr** and exits 1
    /// (`docs/spec/cli.md` §7, §11 Q6 — UNVERIFIED). Compiler diagnostics go to
    /// stdout; only this one class goes to stderr.
    Failed(String),
}

/// Parses `argv[1..]`.
///
/// Zero arguments behaves exactly as `--help=all` (`compiler/hs/app/Main.hs:37`).
pub fn parse(args: &[String]) -> Outcome {
    if args.is_empty() {
        return Outcome::Help(help::help_text(Scope::Program, help::Format::ALL));
    }

    let mut opts = LegacyOptions::default();
    let mut index = 0;

    // The mode word, if present, must come first (`modes [compile &= auto,
    // pretty, listDeps]`, `Options/CmdArgs.hs:167`); anything else there is
    // the program's own (`compile`'s) first argument.
    let mut scope = Scope::Program;
    match lookup_mode(&args[0]) {
        Ok(Some(mode)) => {
            opts.mode = mode;
            scope = Scope::Mode(mode);
            index = 1;
        }
        Ok(None) => {}
        Err(message) => return Outcome::Failed(message),
    }

    let mut positional_only = false;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;

        if positional_only || arg == "-" || !arg.starts_with('-') {
            opts.files.push(arg.to_owned());
            continue;
        }
        if arg == "--" {
            positional_only = true;
            continue;
        }

        let (spec, display, attached) = if let Some(body) = arg.strip_prefix("--") {
            let (name, attached) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (body, None),
            };
            match resolve_long(name, opts.mode) {
                Ok(spec) => (spec, format!("--{name}"), attached),
                Err(message) => return Outcome::Failed(message),
            }
        } else {
            // Short flags: `-O0` and `-O 0` both work, matching `-O0` in
            // `test/logic/CMakeLists.txt:180`.
            let mut chars = arg[1..].chars();
            let letter = chars.next().expect("`-` alone was handled above");
            let rest: String = chars.collect();
            match resolve_short(letter, opts.mode) {
                Some(spec) => {
                    let attached = if rest.is_empty() { None } else { Some(rest) };
                    (spec, format!("-{letter}"), attached)
                }
                None => {
                    return Outcome::Failed(format!(
                        "Unknown flag: {arg}\nTry `kanagawa --help` for the list of flags."
                    ));
                }
            }
        };

        if attached.is_some() && !spec.accepts_attached_value() {
            return Outcome::Failed(format!("Flag {display} does not take a value"));
        }

        // Only value-taking flags reach for the next argv element; switches and
        // optional-value flags never do, so a following file name stays a file.
        let value = match attached {
            Some(value) => Some(value),
            None if spec.takes_separate_value() => {
                let Some(next) = args.get(index) else {
                    return Outcome::Failed(format!("Flag {display} requires an argument"));
                };
                index += 1;
                Some(next.clone())
            }
            None => None,
        };

        match apply(spec, value.as_deref(), &mut opts) {
            Ok(Some(Action::Help(format))) => {
                return match help::Format::parse(format.as_deref().unwrap_or("")) {
                    Ok(format) => Outcome::Help(help::help_text(scope, format)),
                    Err(message) => Outcome::Failed(message),
                };
            }
            Ok(Some(Action::Version)) => return Outcome::Version(help::version_text()),
            Ok(Some(Action::NumericVersion)) => {
                return Outcome::NumericVersion(help::NUMERIC_VERSION.to_owned());
            }
            Ok(None) => {}
            Err(message) => return Outcome::Failed(format!("{display}: {message}")),
        }
    }

    Outcome::Parsed(Box::new(opts))
}

/// The mode a first argument names, by `lookupName`
/// (`Explicit/Process.hs:140-149`): a name it equals, else the one name it
/// is a prefix of — so `l` is `list-deps` and `c` is `compile`, as in the
/// reference, and a file called `p` is a mode word there too. `compile` is
/// also `[compile]`, the name `cmdargs` gives the automatic mode
/// (`Implicit/Global.hs:218`). A prefix of several, which only the empty
/// argument is, is the reference's error.
fn lookup_mode(arg: &str) -> Result<Option<Mode>, String> {
    let names = |mode: Mode| -> &'static [&'static str] {
        match mode {
            Mode::Compile => &["[compile]", "compile"],
            Mode::Pretty => &["pretty"],
            Mode::ListDeps => &["list-deps"],
        }
    };
    let pick = |matches: &dyn Fn(&str) -> bool| -> Vec<(Mode, &'static str)> {
        Mode::ALL.into_iter().filter_map(|m| names(m).iter().copied().find(|n| matches(n)).map(|n| (m, n))).collect()
    };
    let exact = pick(&|n| n == arg);
    let found = if exact.is_empty() { pick(&|n| n.starts_with(arg)) } else { exact };
    match found.as_slice() {
        [] => Ok(None),
        [(mode, _)] => Ok(Some(*mode)),
        many => {
            let names: Vec<&str> = many.iter().map(|(_, n)| *n).collect();
            Err(format!("Ambiguous mode '{arg}', could be any of: {}", names.join(" ")))
        }
    }
}

/// A built-in that ends parsing.
enum Action {
    Help(Option<String>),
    Version,
    NumericVersion,
}

fn apply(
    spec: &FlagSpec,
    value: Option<&str>,
    opts: &mut LegacyOptions,
) -> Result<Option<Action>, String> {
    match spec.sink {
        Sink::Int(field) => *field(opts) = parse_int(value.expect("value flag"))?,
        Sink::Float(field) => *field(opts) = parse_float(value.expect("value flag"))?,
        // A bare switch is `true`; `=false` and friends are honoured.
        Sink::Bool(field) => {
            *field(opts) = match value {
                Some(text) => parse_bool(text)?,
                None => true,
            }
        }
        Sink::Str(field) => *field(opts) = value.expect("value flag").to_owned(),
        // Repeated list flags append, in command-line order (`--import-dir`
        // twice in every logic test, `test/logic/CMakeLists.txt:129-137`).
        Sink::List(field) => field(opts).push(value.expect("value flag").to_owned()),
        Sink::MaybeInt(field) => *field(opts) = Some(parse_int(value.expect("value flag"))?),
        // `&= opt (maxBound::Int)`: bare means "all".
        Sink::OptInt(field) => {
            *field(opts) = match value {
                Some(text) => parse_int(text)?,
                None => i64::MAX,
            }
        }
        Sink::Backend => {
            let text = value.expect("value flag");
            opts.backend =
                Backend::parse(text).ok_or_else(|| format!("unknown backend `{text}` (sv|none)"))?;
        }
        Sink::Layout => {
            let text = value.expect("value flag");
            opts.layout = Layout::parse(text)
                .ok_or_else(|| format!("unknown layout `{text}` (pretty|smart)"))?;
        }
        // The load-bearing one: contributions accumulate on top of the four
        // defaults instead of replacing them (`Warning.hs:46-50` folds the whole
        // list left, and the defaults sit at its head).
        Sink::Warnings(states) => opts.warnings_state.extend_from_slice(states),
        Sink::Builtin(builtin) => {
            return Ok(match builtin {
                Builtin::Help => Some(Action::Help(value.map(str::to_owned))),
                Builtin::Version => Some(Action::Version),
                Builtin::NumericVersion => Some(Action::NumericVersion),
                Builtin::Verbose => {
                    opts.verbosity = Verbosity::Loud;
                    None
                }
                Builtin::Quiet => {
                    opts.verbosity = Verbosity::Quiet;
                    None
                }
            });
        }
    }
    Ok(None)
}

fn parse_int(text: &str) -> Result<i64, String> {
    text.trim().parse::<i64>().map_err(|_| format!("`{text}` is not an integer"))
}

fn parse_float(text: &str) -> Result<f64, String> {
    text.trim().parse::<f64>().map_err(|_| format!("`{text}` is not a number"))
}

/// `--no-implicit-base=false` is the only spelling proved in-tree
/// (`test/syntax/CMakeLists.txt:76`); the rest of this set is the conventional
/// superset (§11 Q8, UNVERIFIED).
fn parse_bool(text: &str) -> Result<bool, String> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(format!("`{text}` is not a boolean (true|false)")),
    }
}

/// Exact match first, then unique-prefix match.
///
/// The prefix rule is what makes `--base=<...>` resolve to `--base-library`
/// (`tools/chkdoc/chkdoc.cpp:208`). An ambiguous prefix is an error that names
/// every candidate, because silently picking one would change compilation
/// results invisibly.
fn resolve_long(name: &str, mode: Mode) -> Result<&'static FlagSpec, String> {
    if let Some(spec) =
        FLAGS.iter().find(|f| f.modes.accepts(mode) && f.long_names().any(|n| n == name))
    {
        return Ok(spec);
    }

    let matches: Vec<&FlagSpec> = FLAGS
        .iter()
        .filter(|f| f.modes.accepts(mode) && f.long_names().any(|n| n.starts_with(name)))
        .collect();

    match matches.as_slice() {
        [spec] => Ok(spec),
        [] => {
            // A flag that exists in another mode gets a better message than
            // "unknown", because `kanagawa pretty --output x` is a plausible
            // mistake with a non-obvious cause.
            let other = Mode::ALL.into_iter().find(|&other| {
                other != mode && FLAGS.iter().any(|f| f.modes.accepts(other) && f.long_names().any(|n| n == name))
            });
            if let Some(other) = other {
                Err(format!(
                    "Unknown flag: --{name}\n`--{name}` is a {} flag; this is {} mode.",
                    other.as_str(),
                    mode.as_str()
                ))
            } else {
                Err(format!(
                    "Unknown flag: --{name}\nTry `kanagawa --help` for the list of flags."
                ))
            }
        }
        many => {
            let names: Vec<String> = many.iter().map(|f| format!("--{}", f.long)).collect();
            Err(format!("Ambiguous flag: --{name} matches {}", names.join(", ")))
        }
    }
}

fn resolve_short(letter: char, mode: Mode) -> Option<&'static FlagSpec> {
    FLAGS.iter().find(|f| f.modes.accepts(mode) && f.short_modes.accepts(mode) && f.short == Some(letter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::options::WarningState;
    use kg_diag::WarningKind;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn parsed(list: &[&str]) -> LegacyOptions {
        match parse(&args(list)) {
            Outcome::Parsed(opts) => *opts,
            other => panic!("expected a parse, got {other:?}"),
        }
    }

    fn failure(list: &[&str]) -> String {
        match parse(&args(list)) {
            Outcome::Failed(message) => message,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn zero_arguments_prints_help() {
        assert!(matches!(parse(&[]), Outcome::Help(_)));
    }

    #[test]
    fn compile_is_the_default_mode_and_files_are_positional() {
        let o = parsed(&["a.k", "b.k"]);
        assert_eq!(o.mode, Mode::Compile);
        assert_eq!(o.files, ["a.k", "b.k"]);

        let o = parsed(&["compile", "a.k"]);
        assert_eq!(o.mode, Mode::Compile);
        assert_eq!(o.files, ["a.k"]);

        let o = parsed(&["pretty", "a.k"]);
        assert_eq!(o.mode, Mode::Pretty);
        assert_eq!(o.files, ["a.k"]);
    }

    /// `lookupName` over the mode names (`Explicit/Process.hs:140-149`): a
    /// prefix names the one mode it starts, `[compile]` is `compile`, and
    /// the empty word, a prefix of all three, is the reference's error
    /// verbatim (probed 2026-10-09). Red with exact matching only.
    #[test]
    fn a_mode_word_is_any_prefix_of_one_mode_s_name() {
        assert_eq!(parsed(&["list-deps", "a.k"]).mode, Mode::ListDeps);
        assert_eq!(parsed(&["l", "a.k"]).mode, Mode::ListDeps);
        assert_eq!(parsed(&["comp", "a.k"]).mode, Mode::Compile);
        assert_eq!(parsed(&["[compile]", "a.k"]).mode, Mode::Compile);
        assert_eq!(parsed(&["pre", "a.k"]).mode, Mode::Pretty);
        assert_eq!(parsed(&["pre", "a.k"]).files, ["a.k"]);
        assert_eq!(failure(&["", "a.k"]), "Ambiguous mode '', could be any of: [compile] pretty list-deps");
    }

    /// `list-deps` takes `ListDeps`' eight fields (`Options.hs:133-141`)
    /// and the letters `cmdargs` gives them there — `-f` is `--file-list`,
    /// where in `compile` it is `--frequency`.
    #[test]
    fn list_deps_takes_its_own_eight_flags_and_letters() {
        let o = parsed(&[
            "list-deps", "-b", "b.k", "-f", "deps.txt", "-n", "-p", "-t", "dev", "-d", "A#1", "-u", "T#uint8",
            "--import-dir=lib", "a.k",
        ]);
        assert_eq!((o.mode, o.base_library.as_str(), o.file_list.as_str(), o.target_device.as_str()), (
            Mode::ListDeps,
            "b.k",
            "deps.txt",
            "dev"
        ));
        assert!(o.no_implicit_base && o.parse_docs);
        assert_eq!((o.define.as_slice(), o.using.as_slice(), o.import_dir.as_slice()), (
            &["A#1".to_owned()][..],
            &["T#uint8".to_owned()][..],
            &["lib".to_owned()][..]
        ));
        assert!(failure(&["list-deps", "--output=x", "a.k"]).contains("is a compile flag"));
        assert!(failure(&["list-deps", "--dump-parse", "a.k"]).contains("is a compile flag"));
        assert_eq!(parsed(&["-f", "250", "a.k"]).frequency, 250);
    }

    /// The help a `--help` prints is the mode's named first, else the
    /// program's — `kanagawa a.k --help` too, whose flags the program's
    /// own mode parses (`processMode`, `Explicit/Process.hs:17-30`).
    #[test]
    fn help_belongs_to_the_mode_named_first() {
        let help = |list: &[&str]| match parse(&args(list)) {
            Outcome::Help(text) => text,
            other => panic!("{other:?}"),
        };
        assert!(help(&["a.k", "--help"]).contains("\nCommands:\n"));
        assert!(help(&["list-deps", "--help"]).contains("\nkanagawa list-deps [OPTIONS] [FILE]\n"));
        assert!(!help(&["list-deps", "--help"]).contains("Commands:"));
        assert!(help(&["--help=all"]).contains("\nkanagawa pretty [OPTIONS] [FILE]\n"));
        assert_eq!(
            failure(&["--help=wide"]),
            "unrecognised help format, expected one of: all one def html text <NUMBER>"
        );
    }

    /// The field is `dump_IR` and `cmdargs` lower-cases what it derives from
    /// a field (`asName`, `Implicit/Global.hs:232`): `--dump-ir`, which this
    /// binary refused until 2026-10-09 while accepting `--dump-IR`, which the
    /// reference refuses.
    #[test]
    fn dump_ir_is_spelled_in_lower_case() {
        assert!(parsed(&["--dump-ir", "a.k"]).dump_ir);
        assert!(failure(&["--dump-IR", "a.k"]).starts_with("Unknown flag: --dump-IR"));
    }

    #[test]
    fn a_mode_word_is_only_a_mode_in_first_position() {
        let o = parsed(&["a.k", "pretty"]);
        assert_eq!(o.mode, Mode::Compile);
        assert_eq!(o.files, ["a.k", "pretty"]);
    }

    #[test]
    fn values_bind_in_both_the_attached_and_the_separate_form() {
        assert_eq!(parsed(&["--reset-cycles=5", "a.k"]).reset_cycles, 5);
        assert_eq!(parsed(&["--reset-cycles", "5", "a.k"]).reset_cycles, 5);
        assert_eq!(parsed(&["--base-library", "helper/custom_base.k"]).base_library, "helper/custom_base.k");
    }

    #[test]
    fn switches_accept_an_explicit_false() {
        assert!(parsed(&["--no-implicit-base", "a.k"]).no_implicit_base);
        assert!(!parsed(&["--no-implicit-base=false", "a.k"]).no_implicit_base);
        assert!(parsed(&["--no-implicit-base=true", "a.k"]).no_implicit_base);
    }

    #[test]
    fn a_switch_never_swallows_the_next_argument() {
        let o = parsed(&["--parse-docs", "a.k"]);
        assert!(o.parse_docs);
        assert_eq!(o.files, ["a.k"], "the file must not be read as the switch's value");
    }

    #[test]
    fn optional_value_flags_mean_max_bound_when_bare() {
        let o = parsed(&["--frontend-passes", "a.k"]);
        assert_eq!(o.passes, i64::MAX);
        assert_eq!(o.files, ["a.k"], "an opt-valued flag must not consume the file");
        assert_eq!(parsed(&["--frontend-passes=0", "a.k"]).passes, 0);
        assert_eq!(parsed(&["--template-iterations=1", "a.k"]).template_iterations, 1);
        assert_eq!(parsed(&["--template-passes=3", "a.k"]).template_passes, 3);
    }

    #[test]
    fn short_flags_take_attached_and_separate_values() {
        assert_eq!(parsed(&["-O0", "a.k"]).optimize, 0);
        assert_eq!(parsed(&["-O", "1", "a.k"]).optimize, 1);
        assert_eq!(parsed(&["-f250", "a.k"]).frequency, 250);
        assert_eq!(parsed(&["-d", "verbose#true", "a.k"]).define, ["verbose#true"]);
        assert_eq!(parsed(&["-u", "T#uint8", "a.k"]).using, ["T#uint8"]);
    }

    /// `cmdargs` auto-assigns short flags mode by mode, and the reference's
    /// own `--help` lists them (2026-09-23): `-j`, `-o` in `compile`;
    /// `-b -i -l -n -p -r -t` in `pretty`, where `--base-library`,
    /// `--no-implicit-base`, `--parse-docs` and `--target-device` get
    /// letters they do not have in `compile`. The output gate runs both
    /// compilers with `-o`, which this binary refused.
    #[test]
    fn auto_assigned_short_flags_are_accepted_in_their_own_mode_only() {
        assert_eq!(parsed(&["-o", "out/x", "a.k"]).output, "out/x");
        assert_eq!(parsed(&["-j2", "a.k"]).jobs, Some(2));
        let pretty = parsed(&["pretty", "-i", "2", "-r", "0.5", "-l", "smart", "-w", "60", "-b", "b.k", "-n", "-p", "-t", "dev", "a.k"]);
        assert_eq!((pretty.indent, pretty.line_width, pretty.base_library.as_str(), pretty.target_device.as_str()), (2, 60, "b.k", "dev"));
        assert!(pretty.no_implicit_base && pretty.parse_docs);
        for flag in ["-b", "-n", "-p", "-t", "-i"] {
            assert!(matches!(parse(&args(&[flag, "x", "a.k"])), Outcome::Failed(_)), "{flag} in compile");
        }
        assert!(matches!(parse(&args(&["pretty", "-o", "x", "a.k"])), Outcome::Failed(_)));
    }

    #[test]
    fn an_unambiguous_prefix_resolves() {
        // The only in-tree evidence of prefix matching: chkdoc.cpp:208.
        assert_eq!(parsed(&["--base=mini-base.k", "a.k"]).base_library, "mini-base.k");
        assert_eq!(parsed(&["--imp=lib", "a.k"]).import_dir, ["lib"]);
    }

    #[test]
    fn an_exact_name_wins_over_the_longer_flags_it_prefixes() {
        // `--stall` prefixes `--stallable-pipelines`, `--target` prefixes
        // `--target-device`, `--define` prefixes nothing but is `explicit`.
        // Exact match must win, or every one of these becomes ambiguous.
        assert_eq!(parsed(&["--stall=2", "a.k"]).stall, 2);
        assert!(!parsed(&["--stall=2", "a.k"]).stallable_pipelines);
        assert_eq!(parsed(&["--target=sv", "a.k"]).backend, Backend::Sv);
        assert_eq!(parsed(&["--target-device=agilex-7-f-023", "a.k"]).target_device, "agilex-7-f-023");
        assert!(parsed(&["--dump-parse", "a.k"]).dump_parse);
        assert_eq!(parsed(&["--max-block-ram=7", "a.k"]).max_block_ram, 7);
    }

    #[test]
    fn a_pretty_only_flag_is_refused_in_compile_mode() {
        let message = failure(&["--layout=smart", "a.k"]);
        assert!(message.contains("pretty flag"), "{message}");
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_and_names_the_candidates() {
        let message = failure(&["--max=1", "a.k"]);
        assert!(message.starts_with("Ambiguous flag: --max matches "), "{message}");
        assert!(message.contains("--max-block-ram"), "{message}");
    }

    #[test]
    fn unknown_flags_fail() {
        assert!(failure(&["--no-such-flag", "a.k"]).starts_with("Unknown flag: --no-such-flag"));
        assert!(failure(&["-Z", "a.k"]).starts_with("Unknown flag: -Z"));
    }

    #[test]
    fn a_compile_only_flag_in_pretty_mode_says_so() {
        let message = failure(&["pretty", "--output=x", "a.k"]);
        assert!(message.contains("compile flag"), "{message}");
    }

    #[test]
    fn pretty_inherits_the_eight_source_resolution_flags() {
        let o = parsed(&["pretty", "--import-dir=lib", "--define=A#1", "--parse-docs", "a.k"]);
        assert_eq!(o.import_dir, ["lib"]);
        assert_eq!(o.define, ["A#1"]);
        assert!(o.parse_docs);
        assert_eq!(o.line_width, 80);
    }

    #[test]
    fn repeated_scalars_are_last_wins() {
        // riscv_sim passes --reset-cycles=5 (BASE) then --reset-cycles=2.
        assert_eq!(parsed(&["--reset-cycles=5", "--reset-cycles=2", "a.k"]).reset_cycles, 2);
    }

    #[test]
    fn repeated_lists_append_in_order() {
        let o = parsed(&["--import-dir=/root/library", "--import-dir", "/root/test/logic", "a.k"]);
        assert_eq!(o.import_dir, ["/root/library", "/root/test/logic"]);
    }

    #[test]
    fn warning_switches_accumulate_over_the_defaults() {
        // shadow-warning.k passes only --Wno-redundant-import and still expects
        // ten `warning:shadow` hits, which only works if the defaults survive.
        let o = parsed(&["--Wno-redundant-import", "a.k"]);
        let set = o.enabled_warnings();
        assert!(set.is_enabled(WarningKind::Shadow));
        assert!(set.is_enabled(WarningKind::Deprecated));
        assert!(set.is_enabled(WarningKind::InitializerOverrides));
        assert!(!set.is_enabled(WarningKind::RedundantImport));
        assert!(!set.is_enabled(WarningKind::Conversion));
    }

    #[test]
    fn the_logic_suite_warning_set_is_the_documented_one() {
        // BASE + LOGIC-COMMON: --Wall --Werror … --Wno-initializer-overrides --Wno-shadow
        let o = parsed(&[
            "--Wall",
            "--Werror",
            "--Wno-initializer-overrides",
            "--Wno-shadow",
            "a.k",
        ]);
        let set = o.enabled_warnings();
        assert!(o.warnings_as_errors);
        assert!(set.is_enabled(WarningKind::RedundantImport));
        assert!(set.is_enabled(WarningKind::Deprecated));
        assert!(set.is_enabled(WarningKind::Conversion));
        assert!(set.is_enabled(WarningKind::TransactionSize));
        assert!(!set.is_enabled(WarningKind::InitializerOverrides));
        assert!(!set.is_enabled(WarningKind::Shadow));
    }

    #[test]
    fn warning_order_matters_and_the_last_switch_wins() {
        assert!(!parsed(&["--Wall", "--Wno-conversion", "a.k"]).enabled_warnings().is_enabled(WarningKind::Conversion));
        assert!(parsed(&["--Wno-conversion", "--Wall", "a.k"]).enabled_warnings().is_enabled(WarningKind::Conversion));
    }

    #[test]
    fn the_state_list_keeps_the_defaults_at_its_head() {
        let o = parsed(&["--Wall", "a.k"]);
        assert_eq!(o.warnings_state.len(), 6);
        assert_eq!(o.warnings_state[0], WarningState::Enable(WarningKind::RedundantImport));
        assert_eq!(o.warnings_state[4], WarningState::Enable(WarningKind::Conversion));
        assert_eq!(o.warnings_state[5], WarningState::Enable(WarningKind::TransactionSize));
    }

    #[test]
    fn a_warning_switch_takes_no_value() {
        assert!(failure(&["--Wall=x", "a.k"]).contains("does not take a value"));
    }

    #[test]
    fn backend_and_layout_values_are_case_insensitive() {
        assert_eq!(parsed(&["--backend=sv", "a.k"]).backend, Backend::Sv);
        assert_eq!(parsed(&["--backend=SV", "a.k"]).backend, Backend::Sv);
        assert_eq!(parsed(&["--target=sv", "a.k"]).backend, Backend::Sv);
        assert_eq!(parsed(&["--backend=none", "a.k"]).backend, Backend::None);
        assert!(failure(&["--backend=vhdl", "a.k"]).contains("unknown backend"));
        assert_eq!(parsed(&["pretty", "--layout=smart", "a.k"]).layout, Layout::Smart);
    }

    #[test]
    fn a_missing_value_is_an_error_not_a_silent_default() {
        assert!(failure(&["--output"]).contains("requires an argument"));
    }

    #[test]
    fn bad_numbers_are_refused() {
        assert!(failure(&["--reset-cycles=x", "a.k"]).contains("not an integer"));
        assert!(failure(&["--mem-to-reg=x", "a.k"]).contains("not a number"));
    }

    #[test]
    fn double_dash_ends_flag_parsing() {
        let o = parsed(&["--", "--not-a-flag.k"]);
        assert_eq!(o.files, ["--not-a-flag.k"]);
    }

    #[test]
    fn verbosity_is_recorded() {
        assert_eq!(parsed(&["-v", "a.k"]).verbosity, Verbosity::Loud);
        assert_eq!(parsed(&["--quiet", "a.k"]).verbosity, Verbosity::Quiet);
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(parse(&args(&["--help"])), Outcome::Help(_)));
        assert!(matches!(parse(&args(&["--help=all"])), Outcome::Help(_)));
        assert!(matches!(parse(&args(&["-?"])), Outcome::Help(_)));
        assert!(matches!(parse(&args(&["--version"])), Outcome::Version(_)));
        assert!(matches!(parse(&args(&["-V"])), Outcome::Version(_)));
        assert!(matches!(parse(&args(&["--numeric-version"])), Outcome::NumericVersion(_)));
    }

    #[test]
    fn the_full_syntax_suite_command_line_parses() {
        // test/syntax/CMakeLists.txt:40-49 + syntax-test.py:103-106.
        let o = parsed(&[
            "--jobs=1",
            "--Wno-redundant-import",
            "--base-library=/root/library/mini-base.k",
            "--import-dir=/root/library",
            "--no-implicit-base=false",
            "--template-iterations=1",
            "--place-iterations=1",
            "--import-dir",
            "/root/test/syntax/base-test",
            "syntax_test_temp_file.k",
        ]);
        assert_eq!(o.jobs, Some(1));
        assert_eq!(o.base_library, "/root/library/mini-base.k");
        assert_eq!(o.import_dir, ["/root/library", "/root/test/syntax/base-test"]);
        assert!(!o.no_implicit_base);
        assert_eq!(o.template_iterations, 1);
        assert_eq!(o.place_iterations, 1);
        assert_eq!(o.files, ["syntax_test_temp_file.k"]);
    }

    #[test]
    fn the_expanded_stall2_logic_suite_command_line_parses() {
        // docs/spec/cli.md §8.2, fully expanded.
        let o = parsed(&[
            "--Wall", "--Werror", "--check-narrowing-assignment", "--parse-docs",
            "--max-block-ram=10000000", "--frequency=250", "--reset-cycles=5",
            "--register-ratio=3", "--backend=sv", "--import-dir=/root/library",
            "--define=verbose#true", "--Wno-initializer-overrides", "--Wno-shadow",
            "--define=ECC_SUPPORT#true", "--using=COMPILE_PARAM_1#uint8",
            "--define=COMPILE_PARAM_2#9", "--define=tag#0", "--import-dir=/root/test/logic",
            "--debug", "--stall=2", "--output", "/build/logic.STALL2.stall/", "stall.k",
        ]);
        assert_eq!(o.max_block_ram, 10_000_000);
        assert_eq!(o.frequency, 250);
        assert_eq!(o.reset_cycles, 5);
        assert_eq!(o.register_ratio, 3);
        assert_eq!(o.backend, Backend::Sv);
        assert_eq!(o.stall, 2);
        assert!(o.debug && o.check_narrowing_assignment && o.parse_docs && o.warnings_as_errors);
        assert_eq!(o.output, "/build/logic.STALL2.stall/");
        assert_eq!(o.files, ["stall.k"]);
        assert_eq!(
            o.defines(),
            [("verbose", "true"), ("ECC_SUPPORT", "true"), ("COMPILE_PARAM_2", "9"), ("tag", "0")]
        );
        assert_eq!(o.usings(), [("COMPILE_PARAM_1", "uint8")]);
    }
}
