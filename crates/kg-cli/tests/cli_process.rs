//! End-to-end tests: real processes, real files, real exit codes.
//!
//! The unit tests drive `run(argv, out, err)` directly; these exist because the
//! contract downstream depends on is a *process* contract — which stream a
//! message lands on and what the shell sees in `$?`. `test/syntax/syntax-test.py`
//! reads stdout and asserts on the exit code, so those two are the interface.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KANAGAWA: &str = env!("CARGO_BIN_EXE_kanagawa");
const KG: &str = env!("CARGO_BIN_EXE_kg");

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("kg-cli-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch dir");
        Scratch(path)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).expect("write scratch file");
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_in(program: &str, dir: &Path, args: &[&str]) -> Run {
    run_in_env(program, dir, args, &[])
}

/// [`run_in`] with environment variables set for the child.
fn run_in_env(program: &str, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let output: Output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .envs(env.iter().copied())
        .output()
        .unwrap_or_else(|e| panic!("failed to run {program}: {e}"));
    Run {
        code: output.status.code().expect("the process should exit normally"),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

const GOOD: &str = "module test{module test}\n\ninline void main()\n{\n}\n";
const BAD: &str = "inline void main(\n{\n";

// ---------------------------------------------------------------- kanagawa

#[test]
fn no_arguments_is_help_on_stdout_with_exit_zero() {
    let scratch = Scratch::new("help");
    let run = run_in(KANAGAWA, scratch.path(), &[]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("Kanagawa Compiler"), "{}", run.stdout);
    assert!(run.stdout.contains("--backend"), "help should list the flags");
    assert!(run.stderr.is_empty(), "{}", run.stderr);
}

#[test]
fn flags_without_files_is_the_reference_message_and_exit_one() {
    let scratch = Scratch::new("nofiles");
    let run = run_in(KANAGAWA, scratch.path(), &["--backend=sv"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.stdout, "Missing source filename(s)\n");
    assert!(run.stderr.is_empty(), "{}", run.stderr);
}

#[test]
fn a_syntax_error_prints_error_one_to_stdout_and_exits_one() {
    let scratch = Scratch::new("syntax");
    scratch.file("bad.k", BAD);
    let run = run_in(KANAGAWA, scratch.path(), &["bad.k"]);
    assert_eq!(run.code, 1);
    // What `syntax-test.py` scrapes: `Error (\d+)` on stdout.
    assert!(run.stdout.starts_with("Error 1: "), "{}", run.stdout);
    assert!(run.stdout.contains("bad.k:"), "{}", run.stdout);
    assert!(run.stderr.is_empty(), "diagnostics must not go to stderr: {}", run.stderr);
}

/// A request for **RTL** with no backend on the machine must not exit 0.
/// `KG_BACKEND_DIR=none` is that machine, whatever this one has.
#[test]
fn a_clean_parse_still_fails_loudly_rather_than_faking_success() {
    let scratch = Scratch::new("clean");
    scratch.file("good.k", GOOD);
    let run = run_in_env(KANAGAWA, scratch.path(), &["--backend=sv", "good.k"], &[("KG_BACKEND_DIR", "none")]);
    assert_eq!(run.code, 1, "a compilation that did not happen must not exit 0");
    assert!(run.stdout.contains("parsed 1 source file"), "{}", run.stdout);
    assert!(run.stdout.contains("not part of this build"), "{}", run.stdout);
    assert!(run.stdout.contains("SystemVerilog backend"), "{}", run.stdout);
    // No fake diagnostics either: nothing that looks like an error code.
    assert!(!run.stdout.contains("Error 1:"), "{}", run.stdout);
}

/// A backend library **named and missing** is a failure, never the
/// frontend-only exit 0 of a machine that has none: the gates name the
/// library they grade against, and a worktree that lacked the compiled-in
/// fallback once graded frontend-only runs as passes (sprint W0.4).
#[test]
fn a_backend_directory_named_without_the_library_fails() {
    let scratch = Scratch::new("named-missing");
    scratch.file("good.k", GOOD);
    let empty = scratch.path().join("no-backend-here");
    std::fs::create_dir_all(&empty).expect("scratch dir");
    let run = run_in_env(KANAGAWA, scratch.path(), &["good.k"], &[("KG_BACKEND_DIR", empty.to_str().expect("UTF-8"))]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("no backend library at"), "{}", run.stderr);
}

/// A clean run with **no backend** exits 0, silently.
///
/// `--backend` defaults to `None` — "produce metadata only"
/// (`Options/CmdArgs.hs:75`) — and that is what `syntax-test.py:103-106`
/// invokes, so every one of upstream's 2,516 live assertions is a
/// frontend-only run expecting exit 0. Exiting 1 here would make this binary
/// useless as a `kanagawa::exe` replacement and forfeit `xtask repoint`.
///
/// Silence matters as much as the code: the runner greps stdout for
/// `Error (\d+)`, so a chatty success is a parsed failure.
#[test]
fn a_clean_frontend_only_run_exits_zero_and_says_nothing() {
    let scratch = Scratch::new("frontend-ok");
    scratch.file("good.k", GOOD);
    let run = run_in_env(KANAGAWA, scratch.path(), &["good.k"], &[("KG_BACKEND_DIR", "none")]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.is_empty(), "a clean run must be silent: {}", run.stdout);
}

/// The backend library's directory and the reference's library tree, when
/// this machine has them: `KG_BACKEND_DIR` where set, else what
/// `scripts/build-backend.sh` builds (`target/backend/dist/bin`); the
/// library under [`kg_base::reference_root`].
fn reference_build() -> Option<(PathBuf, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let backend = std::env::var_os("KG_BACKEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../target/backend/dist/bin"));
    let library = kg_base::reference_root().join("library");
    let lib_file = if cfg!(target_os = "macos") { "libkanagawa-backend.dylib" } else { "libkanagawa-backend.so" };
    if !backend.join(lib_file).is_file() || !library.is_dir() {
        kg_base::corpus_absent(&format!("no reference build at {}", backend.display()));
        return None;
    }
    Some((backend, library))
}

/// A program the backend throws on: `ParseDefaultInitialization` asserts
/// on a `default =` that is not a designated initializer. The reference's
/// frontend rejects it first (`checkDesignatedInitializer`,
/// `TypeCheck.hs:69-71`), and so does ours now, which is why the worker
/// tests below walk it without the frontend's verdict: no program a correct
/// frontend accepts reaches a `Parse*` assertion, so a throw can only be
/// provoked from under the frontend.
const THROWS: &str = "class Main\n{\nprivate:\ndefault = 10;\n}\n";

/// With the reference's backend on this machine, a clean program goes
/// through it: `compile` (`ParseTree.hs:31-41`) end to end, SystemVerilog
/// written, exit 0. Skipped where the build is absent. (The backend's
/// "Running internal consistency checks." line was the evidence until
/// 2026-10-11; a `release`-channel library, `scripts/build-backend.sh`'s
/// default, does not print it.)
#[test]
fn the_compat_binary_compiles_through_the_backend_when_present() {
    let Some((backend, library)) = reference_build() else { return };
    // `mini-base.k`, the syntax tests' base: `base.k`'s closure still holds
    // a template instance the walk declines (`cargo xtask ptdiff` counts it).
    let scratch = Scratch::new("backend-ok");
    // An exported class, so the backend has a module to write: a program
    // with none runs the default pass alone and writes nothing.
    scratch.file("good.k", "class Main\n{\npublic:\n    uint32 get(uint32 x)\n    {\n        return x;\n    }\n}\n\nexport Main;\n");
    let import = format!("--import-dir={}", library.display());
    let base = format!("--base-library={}", library.join("mini-base.k").display());
    let run = run_in_env(
        KANAGAWA,
        scratch.path(),
        &["--backend=sv", "--place-iterations=1", &import, &base, "good.k"],
        &[("KG_BACKEND_DIR", backend.to_str().expect("UTF-8"))],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let written = std::fs::read_dir(scratch.path())
        .expect("scratch dir")
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "sv"));
    assert!(written, "no .sv written: {}{}", run.stdout, run.stderr);
    assert!(!run.stdout.contains("Error "), "{}", run.stdout);
}

/// [`THROWS`] walked into a recording as `kg build` walks a program the
/// frontend passed — here without asking the frontend — and the bytes the
/// worker would be piped.
fn throwing_recording(scratch: &Scratch, library: &Path) -> Vec<u8> {
    let path = scratch.file("throws.k", THROWS);
    let mut settings = kg_config::Settings::fallback(scratch.path().to_str().expect("UTF-8"));
    settings.import_dirs = vec![kg_base::Utf8PathBuf::from(library.to_str().expect("UTF-8"))];
    settings.base_library = Some(kg_base::Utf8PathBuf::from(library.join("mini-base.k").to_str().expect("UTF-8")));
    let db = kg_db::Db::new(settings);
    let root = db.file_id(kg_base::Utf8Path::new(path.to_str().expect("UTF-8")));
    assert!(
        db.check(&[root]).iter().any(|d| d.message.contains("Expected designated initializer")),
        "THROWS is no longer a program the frontend rejects for the reason this test assumes"
    );
    let codegen = kg_ptbridge::program::CodegenArgs {
        target: "None".to_owned(),
        output: "throws".to_owned(),
        resource_usage: String::new(),
        dgml: String::new(),
        dgml_detailed: String::new(),
    };
    let mut recording = kg_ptbridge::record::Recording::new();
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    let outcome = kg_ptbridge::program::run(&db, &[root], options, Some(&codegen), &mut recording);
    assert!(outcome.declined.is_none(), "the walk declined THROWS: {:?}", outcome.declined.map(|d| d.reason));
    recording.encode()
}

/// A C++ exception out of the backend kills the worker, which names the
/// call it came out of (AUDIT D6) — `kanagawa` and `kg` alike, both of
/// which dispatch the worker argument before anything else. Before the
/// worker, this abort was `kanagawa`'s own, "Rust cannot catch foreign
/// exceptions" and nothing more. The parent's half — a signal is
/// `Verdict::Crashed`, never a verdict — is `isolate`'s
/// `a_signal_is_a_crash_by_name`; the exit code a crash is given, 3, is
/// the one arm no test reaches since the frontend closed the only door to
/// a throw.
#[cfg(unix)]
#[test]
fn a_backend_that_throws_is_a_crash_the_worker_names() {
    use std::io::Write as _;
    use std::os::unix::process::ExitStatusExt as _;
    let Some((backend, library)) = reference_build() else { return };
    let scratch = Scratch::new("backend-throws");
    let bytes = throwing_recording(&scratch, &library);
    let lib_file = if cfg!(target_os = "macos") { "libkanagawa-backend.dylib" } else { "libkanagawa-backend.so" };
    for (program, name) in [(KANAGAWA, "kanagawa"), (KG, "kg")] {
        let mut child = Command::new(program)
            .arg(kg_ptbridge::isolate::WORKER_ARG)
            .arg(backend.join(lib_file))
            .current_dir(scratch.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to run {program}: {e}"));
        child.stdin.take().expect("piped").write_all(&bytes).expect("the worker reads its recording");
        let output = child.wait_with_output().expect("the worker ends");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.signal(), Some(6), "{name}: {:?}\n{stderr}", output.status);
        assert!(
            stderr.contains(&format!("{name}: the backend threw a C++ exception out of ParseDefaultInitialization (call ")),
            "{name}: {stderr}"
        );
        assert_eq!(kg_ptbridge::isolate::verdict(output.status), kg_ptbridge::isolate::Verdict::Crashed {
            how: "killed by signal 6, SIGABRT".to_owned()
        });
    }
}

/// And the compat binary runs the **frontend**, not just the parser.
///
/// The fixture parses cleanly and is still wrong (`tNot` has no integer clause,
/// `Type.hs:571`), so a parse-only build reports nothing and exits 0.
#[test]
fn the_compat_binary_reports_semantic_errors() {
    let scratch = Scratch::new("compat-semantic");
    scratch.file("bad.k", "inline void main(uint8 a)\n{\n    bool b = !a;\n}\n");
    let run = run_in(KANAGAWA, scratch.path(), &["bad.k"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.starts_with("Error 1: "), "{}", run.stdout);
    assert!(run.stdout.contains("Expected a boolean operand"), "{}", run.stdout);
}

#[test]
fn an_unknown_flag_goes_to_stderr_and_exits_one() {
    let scratch = Scratch::new("badflag");
    scratch.file("good.k", GOOD);
    let run = run_in(KANAGAWA, scratch.path(), &["--no-such-flag", "good.k"]);
    assert_eq!(run.code, 1);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(run.stderr.contains("Unknown flag"), "{}", run.stderr);
}

#[test]
fn backend_option_validation_is_reproduced_on_stdout() {
    let scratch = Scratch::new("validate");
    scratch.file("good.k", GOOD);
    let run =
        run_in(KANAGAWA, scratch.path(), &["--reset-fan-out-cycles=9", "--reset-cycles=3", "good.k"]);
    assert_eq!(run.code, 1);
    // `config.cpp:162-165`, printed by `e.what()` with no `Error N:` prefix.
    assert_eq!(run.stdout, "--reset-fan-out-cycles must be no greater than --reset-cycles\n");
}

/// A file not found is an exception the reference never catches
/// (`Parser/IO.hs:83`): GHC prints it on stderr, after the program's name,
/// with the search path — stdout stays empty (probed 2026-10-09; this test
/// asserted an `Error 1:` on stdout until then).
#[test]
fn a_missing_file_names_the_search_path_on_stderr() {
    let scratch = Scratch::new("missing");
    let run = run_in(KANAGAWA, scratch.path(), &["--no-implicit-base", "--import-dir=/nowhere", "absent.k"]);
    assert_eq!(run.code, 1);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(run.stderr.starts_with("kanagawa: Can't find file: absent.k"), "{}", run.stderr);
    assert!(run.stderr.contains("\n/nowhere/device/mock\n/nowhere\n"), "{}", run.stderr);
}

#[test]
fn version_and_numeric_version_exit_zero_on_stdout() {
    let scratch = Scratch::new("version");
    for flag in ["--version", "-V", "--numeric-version"] {
        let run = run_in(KANAGAWA, scratch.path(), &[flag]);
        assert_eq!(run.code, 0, "{flag}");
        assert!(run.stdout.contains("0.0.0"), "{flag}: {}", run.stdout);
    }
}

/// The generated RTL's header records the command line as `basename
/// argv[0]` and the arguments as given (`Main.hs:41`'s `unwords $ prog :
/// args`). Until 2026-10-10 this read a line `--verbose` printed, which the
/// reference never prints (`verbose_adds_no_line_of_the_frontend_s`); the
/// header is where the string lands. Skipped where the build is absent.
#[test]
fn the_generated_rtl_records_the_command_line() {
    let Some((backend, library)) = reference_build() else { return };
    let scratch = Scratch::new("cmdargs");
    scratch.file("main.k", "class Main\n{\npublic:\n    uint32 get(uint32 x)\n    {\n        return x;\n    }\n}\n\nexport Main;\n");
    std::fs::create_dir_all(scratch.path().join("o")).expect("scratch directory");
    let import = format!("--import-dir={}", library.display());
    let base = format!("--base-library={}", library.join("mini-base.k").display());
    let args = ["--backend=sv", "--place-iterations=1", base.as_str(), import.as_str(), "--output=o/x", "main.k"];
    let run = run_in_env(KANAGAWA, scratch.path(), &args, &[("KG_BACKEND_DIR", backend.to_str().expect("UTF-8"))]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let sv = std::fs::read_to_string(scratch.path().join("o/xMain.sv")).expect("the backend wrote the RTL");
    let recorded = format!("// kanagawa {}\n", args.join(" "));
    assert!(sv.contains(&recorded), "the recorded string is basename(argv[0]) + argv[1..]:\n{}", &sv[..sv.len().min(400)]);
}

/// `pretty` is refused by decision (`kg fmt` is the formatter), on
/// stderr: what `pretty` writes to stdout is the program, so a refusal there
/// would read as its output.
#[test]
fn pretty_mode_refuses_rather_than_printing_unformatted_source() {
    let scratch = Scratch::new("pretty");
    scratch.file("good.k", GOOD);
    let run = run_in(KANAGAWA, scratch.path(), &["pretty", "good.k"]);
    assert_eq!(run.code, 1);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(run.stderr.contains("`pretty` is not implemented"), "{}", run.stderr);
    assert!(run.stderr.contains("kg fmt"), "{}", run.stderr);
}

/// `list-deps` is a mode a user can type: the program's files, sorted, on
/// stdout (`Main.hs:111-125`).
#[test]
fn list_deps_prints_the_program_s_files() {
    let scratch = Scratch::new("list-deps");
    scratch.file("good.k", GOOD);
    let run = run_in(KANAGAWA, scratch.path(), &["list-deps", "good.k"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let good = std::fs::canonicalize(scratch.path().join("good.k")).expect("written above");
    assert_eq!(run.stdout, format!("{}\n", good.display()));
}

#[cfg(unix)]
#[test]
fn the_kg_binary_dispatches_on_argv0() {
    // docs/spec/cli.md §9.4: installed as `kanagawa`, it must *be* `kanagawa`.
    let scratch = Scratch::new("argv0");
    let link = scratch.path().join("kanagawa");
    std::os::unix::fs::symlink(KG, &link).expect("symlink");
    let run = run_in(link.to_str().unwrap(), scratch.path(), &["--backend=sv"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.stdout, "Missing source filename(s)\n");
}

// ---------------------------------------------------------------------- kg

#[test]
fn kg_dump_cst_prints_a_tree() {
    let scratch = Scratch::new("dump");
    scratch.file("good.k", GOOD);
    let run = run_in(KG, scratch.path(), &["dump", "cst", "good.k"]);
    assert_eq!(run.code, 0);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines[0], format!("SOURCE_FILE@0..{}", GOOD.len()));
    assert!(lines.iter().any(|l| l.starts_with("  ")), "the tree should be indented");
    assert!(run.stdout.contains("IDENT@"), "{}", run.stdout);
    assert!(run.stdout.contains("\"main\""), "{}", run.stdout);
}

#[test]
fn kg_dump_reads_a_real_corpus_file_when_one_is_available() {
    // The corpus is read-only and optional; skip cleanly when it is absent, the
    // same way the parser's corpus tests do.
    let Some(root) = corpus_root() else {
        kg_base::corpus_absent("no corpus; set KANAGAWA_ROOT");
        return;
    };
    let file = root.join("library/mini-base.k");
    if !file.exists() {
        kg_base::corpus_absent(&format!("no corpus file {}", file.display()));
        return;
    }
    let scratch = Scratch::new("corpus");
    let run = run_in(KG, scratch.path(), &["dump", "cst", file.to_str().unwrap()]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.starts_with("SOURCE_FILE@0.."), "{}", run.stdout);
    assert!(run.stdout.lines().count() > 20, "a real file should produce a real tree");
    assert!(!run.stdout.contains("syntax error"), "mini-base.k should parse cleanly");
}

fn corpus_root() -> Option<PathBuf> {
    let root = kg_base::reference_root();
    root.join("library").is_dir().then(|| root.canonicalize().unwrap_or(root))
}

#[test]
fn kg_dump_tokens_and_ast_work_too() {
    let scratch = Scratch::new("dump2");
    scratch.file("good.k", GOOD);
    let tokens = run_in(KG, scratch.path(), &["dump", "tokens", "good.k"]);
    assert_eq!(tokens.code, 0);
    assert!(tokens.stdout.contains("MODULE_KW@0..6"), "{}", tokens.stdout);

    let ast = run_in(KG, scratch.path(), &["dump", "ast", "good.k"]);
    assert_eq!(ast.code, 0);
    assert!(!ast.stdout.contains("WHITESPACE"), "{}", ast.stdout);
}

#[test]
fn kg_check_reports_syntax_errors_and_exits_one() {
    let scratch = Scratch::new("check");
    scratch.file("bad.k", BAD);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "check", "bad.k"]);
    assert_eq!(run.code, 1);
    assert!(run.stdout.contains("error:"), "{}", run.stdout);
    assert!(run.stdout.contains("--> bad.k:"), "{}", run.stdout);
}

/// A file is one module however its path is spelled: the reference knows
/// each by `canonicalizePath` (`Parser/IO.hs:104`). `m.k` named relatively
/// and also imported by `base.k` through an absolute, a relative or a
/// symlinked `-I` was two modules, and its one error was reported twice
/// (issue #4).
#[test]
fn kg_check_loads_a_file_once_however_it_is_spelled() {
    let scratch = Scratch::new("check-spelling");
    std::fs::create_dir_all(scratch.path().join("lib")).unwrap();
    scratch.file("lib/base.k", "module base { module m }\n\nimport m\n");
    scratch.file("lib/m.k", "module m { F }\n\ninline void F()\n{\n    uint32 x = y;\n}\n");
    std::os::unix::fs::symlink(scratch.path().join("lib"), scratch.path().join("link")).unwrap();
    let absolute = scratch.path().join("lib");
    for dir in [absolute.to_str().unwrap(), "lib", "link"] {
        let run = run_in(KG, scratch.path(), &["--no-manifest", "check", "-I", dir, "lib/m.k"]);
        assert_eq!(run.code, 1, "-I {dir}: {}{}", run.stdout, run.stderr);
        assert_eq!(run.stdout.matches("-->").count(), 1, "-I {dir}: {}", run.stdout);
        assert!(run.stdout.contains("--> lib/m.k:5:16"), "-I {dir}: {}", run.stdout);
    }
    // A manifest found from the relative `lib`: the base library is looked
    // for under its directory first (`program::load`), so that is canonical too.
    scratch.file("lib/kanagawa.toml", "[project]\nimport-dirs = [\".\"]\n");
    let run = run_in(KG, scratch.path(), &["check", "lib/m.k"]);
    assert_eq!(run.code, 1, "manifest: {}{}", run.stdout, run.stderr);
    assert_eq!(run.stdout.matches("-->").count(), 1, "manifest: {}", run.stdout);
}

#[test]
fn kg_check_of_a_clean_file_exits_zero_and_says_what_it_checked() {
    let scratch = Scratch::new("check-ok");
    scratch.file("good.k", GOOD);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "check", "good.k"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("no problems found"), "{}", run.stdout);
    // The honesty requirement: it must not imply the reference would agree —
    // the C++ backend's verdict is `build`'s (since 2026-10-10 the frontend's
    // own passes are complete, `conformance --strict` at 0).
    assert!(run.stdout.contains("kg build"), "{}", run.stdout);
}

/// `check` loads the implicit base library, and says so when it cannot find it
/// rather than checking the file as if the base were empty.
///
/// The reference aborts here — `either (Ex.throw . FileException) return`
/// (`Parser/IO.hs:83`) — so reporting an error is the faithful outcome, not an
/// over-eager one. Declaring `import-dirs` without a base library is what
/// triggers it: with *no* import directory at all the implicit base degrades to
/// the synthetic `.cmdargs.k`, which always resolves.
#[test]
fn kg_check_reports_a_missing_base_library() {
    let scratch = Scratch::new("check-nobase");
    scratch.file("kanagawa.toml", "[project]\nimport-dirs = [\"lib\"]\n");
    scratch.file("good.k", GOOD);
    let run = run_in(KG, scratch.path(), &["check", "good.k"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("Can't find file: base.k"), "{}", run.stdout);
}

#[test]
fn kg_check_walks_directories_and_honours_the_manifest() {
    let scratch = Scratch::new("check-dir");
    // `implicit-base = false` keeps the fixture a self-contained program: with
    // an `import-dirs` declared and no `lib/base.k` on disk, the implicit base
    // would fail to resolve — see `kg_check_reports_a_missing_base_library`.
    scratch.file(
        "kanagawa.toml",
        "[project]\nsources = [\"good.k\"]\nimport-dirs = [\"lib\"]\nimplicit-base = false\n",
    );
    scratch.file("good.k", GOOD);
    // With no paths at all, `project.sources` is the input.
    let run = run_in(KG, scratch.path(), &["check"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("checked 1 file"), "{}", run.stdout);

    // `-vv` explains the resolved configuration.
    let run = run_in(KG, scratch.path(), &["-vv", "check"]);
    assert!(run.stdout.contains("import search path"), "{}", run.stdout);
    assert!(run.stdout.contains("kanagawa.toml"), "{}", run.stdout);
}

#[test]
fn kg_check_short_format_is_one_line_per_diagnostic() {
    let scratch = Scratch::new("check-short");
    scratch.file("bad.k", BAD);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "--error-format=short", "check", "bad.k"]);
    assert_eq!(run.code, 1);
    for line in run.stdout.lines() {
        assert!(line.contains(": error: ") || line.contains(": warning: "), "{line}");
    }
}

#[test]
fn kg_check_json_format_is_one_object_per_line() {
    let scratch = Scratch::new("check-json");
    scratch.file("bad.k", BAD);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "--error-format=json", "check", "bad.k"]);
    assert_eq!(run.code, 1);
    for line in run.stdout.lines() {
        assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
        assert!(line.contains("\"severity\":"), "{line}");
    }
}

#[test]
fn kg_fmt_checks_then_writes_and_the_second_run_is_a_no_op() {
    let scratch = Scratch::new("fmt");
    let messy = "class C\n{\npublic:\n    void run(unit::tag_t tag)    {\n        f(tag);\n    }\n}\n";
    let tidy = "class C\n{\npublic:\n    void run(unit::tag_t tag)\n    {\n        f(tag);\n    }\n}\n";
    let path = scratch.file("messy.k", messy);

    // `--check` names the file on stdout, exits 1, and writes nothing.
    let run = run_in(KG, scratch.path(), &["--no-manifest", "fmt", "--check", "messy.k"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("messy.k"), "{}", run.stdout);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), messy);

    // `--stdout` writes nothing either.
    let run = run_in(KG, scratch.path(), &["--no-manifest", "fmt", "--stdout", "messy.k"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, tidy);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), messy);

    // The rewrite, and then a clean check on the same file.
    let run = run_in(KG, scratch.path(), &["--no-manifest", "fmt", "messy.k"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), tidy);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "fmt", "--check", "messy.k"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

#[test]
fn kg_fmt_never_rewrites_a_file_that_does_not_parse() {
    let scratch = Scratch::new("fmt-bad");
    let path = scratch.file("bad.k", BAD);
    let run = run_in(KG, scratch.path(), &["--no-manifest", "fmt", "bad.k"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("bad.k"), "{}", run.stderr);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BAD);
}

/// `build` is the last unimplemented command; `lsp` used to be listed here and
/// is covered by `kg_lsp_serves_a_session_over_stdio` instead.
///
/// The stdout assertion is the load-bearing one: exit 2 must arrive with an
/// empty stdout, because `test/syntax/syntax-test.py` reads stdout and would
/// otherwise parse a refusal as compiler output.
#[test]
fn kg_build_without_a_backend_says_where_to_find_one() {
    let scratch = Scratch::new("nobackend");
    scratch.file("good.k", GOOD);
    let args = ["--no-manifest", "build", "--backend=sv", "good.k"];
    let run = run_in_env(KG, scratch.path(), &args, &[("KG_BACKEND_DIR", "none")]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.is_empty(), "wrote to stdout: {}", run.stdout);
    assert!(run.stderr.contains("KG_BACKEND_DIR"), "{}", run.stderr);
}

/// `kg build -o out/sub/demo` makes `out/sub` first: the backend opens
/// its files under the base name with no directory of its own making, and
/// a missing one was an iostream failure from C++ (2026-10-10). Red with
/// the directory left to the backend. Skipped where the build is absent.
#[test]
fn kg_build_makes_the_output_directory() {
    let Some((backend, library)) = reference_build() else { return };
    let scratch = Scratch::new("build-outdir");
    scratch.file("main.k", "class Main\n{\npublic:\n    uint32 get(uint32 x)\n    {\n        return x;\n    }\n}\n\nexport Main;\n");
    let import = format!("-I={}", library.display());
    let base = format!("--base-library={}", library.join("mini-base.k").display());
    let args = ["--no-manifest", "build", &import, &base, "-o", "out/sub/demo", "main.k"];
    let run = run_in_env(KG, scratch.path(), &args, &[("KG_BACKEND_DIR", backend.to_str().expect("UTF-8"))]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(scratch.path().join("out/sub/demoMain.sv").is_file(), "{}{}", run.stdout, run.stderr);

    // `-o dir/` names the directory itself, which the README's quick start
    // writes (2026-10-11): red with only the parent of `dir/` made.
    let args = ["--no-manifest", "build", &import, &base, "-o", "fresh/", "main.k"];
    let run = run_in_env(KG, scratch.path(), &args, &[("KG_BACKEND_DIR", backend.to_str().expect("UTF-8"))]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(scratch.path().join("fresh/Main.sv").is_file(), "{}{}", run.stdout, run.stderr);
}

#[test]
fn kg_explain_knows_the_frontend_code_and_the_backend_names() {
    let scratch = Scratch::new("explain");
    let run = run_in(KG, scratch.path(), &["explain", "1"]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("the frontend rejected the program"), "{}", run.stdout);

    let run = run_in(KG, scratch.path(), &["explain", "E27"]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("InvalidCall"), "{}", run.stdout);

    let run = run_in(KG, scratch.path(), &["explain", "shadow"]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.contains("-W no-shadow"), "{}", run.stdout);

    let run = run_in(KG, scratch.path(), &["explain", "--list"]);
    assert_eq!(run.code, 0);
    assert!(run.stdout.lines().count() > 70, "{}", run.stdout);
}

#[test]
fn kg_usage_errors_exit_two() {
    let scratch = Scratch::new("usage");
    for args in [vec!["frobnicate"], vec!["dump", "graph", "x.k"], vec!["explain"]] {
        let run = run_in(KG, scratch.path(), &args);
        assert_eq!(run.code, 2, "{args:?}");
        assert!(!run.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn kg_help_and_version_exit_zero() {
    let scratch = Scratch::new("kg-help");
    for args in [vec!["--help"], vec!["--version"], vec!["check", "--help"]] {
        let run = run_in(KG, scratch.path(), &args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
        assert!(!run.stdout.is_empty(), "{args:?}");
    }
}

// ---------------------------------------------------------------------------
// `kg lsp`
// ---------------------------------------------------------------------------

/// One LSP frame.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

/// Drives `kg lsp` as a real subprocess through a whole session, with a
/// watchdog.
///
/// The watchdog is the point. The bug this guards against was a **deadlock**,
/// not a wrong answer: `bin/kg.rs` held a `StdoutLock` across dispatch, and
/// `lsp-server`'s writer thread blocked forever trying to lock stdout. A test
/// that merely waited would hang the suite instead of failing it, so the child
/// is killed and the test fails with a message that names the cause.
fn lsp_session(messages: &str) -> String {
    use std::io::{Read, Write};
    use std::process::Stdio;

    let mut child = std::process::Command::new(KG)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn kg lsp");

    child.stdin.take().expect("stdin").write_all(messages.as_bytes()).expect("write");

    let mut stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(out) => {
            let _ = child.wait();
            out
        }
        Err(_) => {
            let _ = child.kill();
            panic!(
                "`kg lsp` produced no complete response in 30s. \
                 The usual cause is the process holding a StdoutLock across dispatch, \
                 which deadlocks lsp-server's writer thread."
            );
        }
    }
}

/// `kg lsp` must actually start the server.
///
/// It did not, for the whole time the server existed: `kg-cli` had no `kg-lsp`
/// dependency and the subcommand exited 2 — while the VS Code client's default
/// `server.path`/`server.args` are exactly `kg` and `["lsp"]`.
#[test]
fn kg_lsp_serves_a_session_over_stdio() {
    let session = format!(
        "{}{}{}{}",
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#),
        frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
        frame(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#),
        frame(r#"{"jsonrpc":"2.0","method":"exit","params":null}"#),
    );
    let out = lsp_session(&session);
    assert!(out.contains("\"capabilities\""), "no initialize result: {out}");
    assert!(out.contains("documentSymbolProvider"), "{out}");
}

/// And it must serve *frontend* diagnostics, not just parse errors.
///
/// The fixture parses cleanly and is still wrong — `tNot` has no clause for an
/// integer (`Type.hs:571`) — so a server wired only to the parser reports
/// nothing here.
#[test]
fn kg_lsp_publishes_semantic_diagnostics() {
    let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/kg-lsp-it.k","languageId":"kanagawa","version":1,"text":"inline void main(uint8 a)\n{\n    bool b = !a;\n}\n"}}}"#;
    let session = format!(
        "{}{}{}{}{}",
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#),
        frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
        frame(open),
        frame(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#),
        frame(r#"{"jsonrpc":"2.0","method":"exit","params":null}"#),
    );
    let out = lsp_session(&session);
    assert!(out.contains("publishDiagnostics"), "no diagnostics published: {out}");
    assert!(out.contains("Expected a boolean operand"), "{out}");
}

/// And the editor's types are the program-wide loop's, the way a user meets
/// them: a hover and an inlay hint inside a template's body give each
/// instance's width — `w` is `v + 1`, `uint9` in `take<uint8>` and `uint17`
/// in `take<uint16>` (`kg-oracle --no-base --passes 19`, 2026-10-10) — and
/// in a plain function the width an instantiated call returns (`r`,
/// `Box<4>::first()`'s `uint33`), which the CST engine the editor read
/// before types in neither place.
#[test]
fn kg_lsp_serves_the_loop_s_types_in_a_template_body_and_a_plain_function() {
    let text = "template <typename T>\ninline void take(T v)\n{\n    auto w = v + 1;\n}\n\n\
                template <auto N>\nclass Box\n{\n    uint32[N] items;\n\n    inline auto first()\n    {\n        return items[0] + N;\n    }\n}\n\n\
                inline void main()\n{\n    take<uint8>(1);\n    take<uint16>(2);\n    static Box<4> a;\n    auto r = a.first();\n}\n";
    let uri = "file:///tmp/kg-lsp-loop.k";
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"kanagawa","version":1,"text":"{}"}}}}}}"#,
        text.replace('\n', "\\n")
    );
    let hover = |id: u32, line: u32, character: u32| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        )
    };
    let hints = format!(
        r#"{{"jsonrpc":"2.0","id":4,"method":"textDocument/inlayHint","params":{{"textDocument":{{"uri":"{uri}"}},"range":{{"start":{{"line":0,"character":0}},"end":{{"line":24,"character":0}}}}}}}}"#
    );
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#),
        frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
        frame(&open),
        frame(&hover(2, 3, 9)),
        frame(&hover(3, 22, 9)),
        frame(&hints),
        frame(r#"{"jsonrpc":"2.0","id":5,"method":"shutdown","params":null}"#),
        frame(r#"{"jsonrpc":"2.0","method":"exit","params":null}"#),
    ]
    .concat();
    let out = lsp_session(&session);
    // Inside the template's body: one line per instance's type.
    assert!(out.contains(r"uint9 w   // take<uint8>\nuint17 w  // take<uint16>"), "{out}");
    assert!(out.contains(r#""label":": uint9 | uint17""#), "{out}");
    // In the plain function: the type the instantiated call returns.
    assert!(out.contains(r"```kanagawa\nuint33 r\n```"), "{out}");
    assert!(out.contains(r#""label":": uint33""#), "{out}");
}
