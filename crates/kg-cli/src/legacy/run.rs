//! Dispatch and exit codes for the compat binary.
//!
//! Two rules govern everything here (`docs/spec/cli.md` §7):
//!
//! * **Diagnostics go to stdout**, because `exitError` is `putStrLn`
//!   (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:116-119`) and
//!   `test/syntax/syntax-test.py` reads stdout. What goes to stderr is what
//!   the reference never catches: `cmdargs`' argv failures, and a file
//!   exception — a source or base library not found (`Parser/IO.hs:83`).
//! * **The only exit codes are 0 and 1** — and [`BACKEND_CRASHED`], which
//!   is not a verdict: the reference has no such exit, because it has no
//!   such outcome; its process dies with the backend.

use std::io::Write;

use kg_base::LineIndex;
use kg_diag::{Diagnostic, Located, SourceView, render_legacy_located};
use kg_ptbridge::isolate::Verdict;

use super::options::{LegacyOptions, Mode, Verbosity};
use super::{cmd_args_string, parse, prog_name, validate};

/// The exit code of a run whose backend crashed (`kg_ptbridge::isolate`):
/// neither 0 nor 1, so `syntax-test.py`, which wants exactly one of those
/// (`:151-156`), fails it whatever the program was expected to do, and so
/// does anything else that reads the code. The reference, in the same
/// place, dies of `SIGABRT` — 134 to a shell.
pub const BACKEND_CRASHED: i32 = 3;

/// The frontend's passes: `--frontend-passes` below this runs a prefix of
/// them (`drop (length passes - frontendPasses) passes`, `Frontend.hs:38,54-76`).
pub const FRONTEND_PASSES: i64 = 19;

/// `instantiateTemplates`' sub-passes: `--template-passes` below this runs a
/// prefix of them in the last round (`Template.hs:62-64,71-81`).
pub const TEMPLATE_SUBPASSES: i64 = 9;

/// Runs the compat front end. Returns the process exit code.
///
/// `out` is stdout and `err` is stderr; both are injected so the whole binary
/// is testable without spawning a process.
pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let cmd_args = cmd_args_string(argv);
    // GHC's uncaught-exception handler names the program by `getProgName`.
    let prog = prog_name(argv.first().map_or("kanagawa", String::as_str));

    let outcome = parse::parse(argv.get(1..).unwrap_or(&[]));
    let opts = match outcome {
        parse::Outcome::Help(text) => {
            let _ = write!(out, "{text}");
            return 0;
        }
        parse::Outcome::Version(text) | parse::Outcome::NumericVersion(text) => {
            let _ = writeln!(out, "{}", text.trim_end());
            return 0;
        }
        parse::Outcome::Failed(message) => {
            let _ = writeln!(err, "kanagawa: {message}");
            return 1;
        }
        parse::Outcome::Parsed(opts) => *opts,
    };

    // `Main.hs:38-39`: an empty positional list is fatal, with no `Error N:`
    // prefix, on stdout — in every mode.
    if opts.files.is_empty() {
        let _ = writeln!(out, "Missing source filename(s)");
        return 1;
    }

    match opts.mode {
        Mode::ListDeps => list_deps(&opts, prog, out, err),
        Mode::Pretty => pretty(&opts, prog, out, err),
        Mode::Compile => {
            if let Some(refusal) = unhonoured_pass_bounds(&opts) {
                let _ = writeln!(err, "{prog}: {refusal}");
                return 1;
            }
            missing_dumps(&opts, prog, err);
            match compile(&opts, &cmd_args, prog, out, err) {
                // The backend ran and said nothing: the reference's clean exit.
                Ok((_, true)) => 0,
                Ok((parsed, false)) => not_implemented(&opts, parsed, &cmd_args, out),
                Err(code) => code,
            }
        }
    }
}

/// `--template-iterations` as the loop's bound (`Template.hs:53-64`): `n`
/// rounds of instantiation where `n` is a count — a round is the pass list,
/// the census, and `instantiate` (`kg_sem::elaborate`), so the reference's
/// `n` recursions are `n` of ours, and `0` is the pass list alone. A
/// negative count never reaches the reference's `== 0` and runs to the
/// fixpoint, as `maxBound` (the default) does.
pub fn pass_budget(opts: &LegacyOptions) -> kg_db::PassBudget {
    match opts.template_iterations {
        n if (0..i64::MAX).contains(&n) => kg_db::PassBudget::AtMost(u32::try_from(n).unwrap_or(u32::MAX)),
        _ => kg_db::PassBudget::Fixpoint,
    }
}

/// The two pass bounds this compiler cannot stop at, refused by name
/// rather than run past (`docs/known-divergences.md` #35).
///
/// `--frontend-passes` below 19 runs a prefix of the frontend
/// (`Frontend.hs:38`) and then hands the backend a tree desugared that far,
/// which no pass boundary here produces. `--template-passes` below 9 runs a
/// prefix of the pass list in the last round of a bounded loop
/// (`Template.hs:62-64`) — one that `--template-iterations` bounds, so it is
/// refused only with a count; under the default bound it does nothing in
/// the reference either. Our rounds run their passes fused
/// (`kg_hir::rewrite::round_prefix`), so neither prefix is a state this
/// compiler ever holds.
pub fn unhonoured_pass_bounds(opts: &LegacyOptions) -> Option<String> {
    if opts.passes < FRONTEND_PASSES {
        return Some(format!(
            "--frontend-passes={} is not supported by this build: it stops the frontend after a prefix of its \
             {FRONTEND_PASSES} passes, a state this compiler never holds (docs/known-divergences.md #35)",
            opts.passes
        ));
    }
    let bounded = (0..i64::MAX).contains(&opts.template_iterations);
    if bounded && opts.template_passes < TEMPLATE_SUBPASSES {
        return Some(format!(
            "--template-passes={} is not supported by this build: it runs a prefix of template instantiation's \
             {TEMPLATE_SUBPASSES} sub-passes in the last round, which this compiler runs fused \
             (docs/known-divergences.md #35)",
            opts.template_passes
        ));
    }
    None
}

/// The `--dump-*` flags that print the reference's own data structures:
/// said on stderr, and the compile goes on without them
/// (`docs/known-divergences.md` #36, #37). `--dump-ir` and `--dump-opt` are
/// the backend's, and work.
fn missing_dumps(opts: &LegacyOptions, prog: &str, err: &mut dyn Write) {
    for (on, flag) in [(opts.dump_parse, "--dump-parse"), (opts.dump_types, "--dump-types"), (opts.dump_program, "--dump-program")] {
        if on {
            let _ = writeln!(
                err,
                "{prog}: {flag} is not implemented in this build: it prints GHC's `show` of the reference's own \
                 syntax tree (`prettyExp`), which this compiler does not have (docs/known-divergences.md #36); \
                 compiling without it"
            );
        }
    }
    if opts.dump_source {
        let _ = writeln!(
            err,
            "{prog}: --dump-source is not implemented, by decision: it is the reference's pretty printer, and \
             `kg fmt` is this project's formatter (docs/known-divergences.md #37); compiling without it"
        );
    }
}

/// What every mode does first (`parseProgram`, `Parser/IO.hs:67-77`): the
/// database the options describe, and each command-line file found — the
/// base library before them. A file that is not found is not a diagnostic
/// there but an exception `parseFile` throws (`:83`) and nothing catches:
/// GHC prints `<prog>: <message>` on stderr and exits 1, at the first one.
fn load(opts: &LegacyOptions, prog: &str, err: &mut dyn Write) -> Result<(kg_db::Db, Vec<kg_base::FileId>), i32> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_owned());

    let mut settings = super::options::settings_from(opts, &cwd);
    // The port's verdict switches, which no flag of the reference's spells
    // (`crate::verdicts_from_env`).
    settings.verdicts = crate::verdicts_from_env();
    let mut db = kg_db::Db::new(settings);
    db.set_pass_budget(pass_budget(opts));

    let missing = |file: &str, err: &mut dyn Write| {
        let dirs: String = opts.import_dirs().iter().map(|d| format!("{d}\n")).collect();
        let _ = writeln!(err, "{prog}: Can't find file: {file} in any of the import directories\n{dirs}");
        1
    };
    // A file found that does not read — not UTF-8, say — is the same
    // uncaught exception, `readFileUtf8`'s own (`Parser/IO.hs:106-107`). Until
    // 2026-10-10 it read here as an empty file, compiled clean and exited 0.
    let unreadable = |found: &Found, err: &mut dyn Write| {
        let file = db.file_id(kg_base::Utf8Path::new(&found.canonical));
        let failure = db.vfs().read_failure(file)?;
        let error = kg_resolve::module::read_failed(kg_base::Utf8Path::new(&found.joined), failure);
        let _ = writeln!(err, "{prog}: {error}");
        Some(1)
    };
    // `importBase`: the base library, unless `.cmdargs` stands in for it
    // (`Parser/IO.hs:73-75`).
    if !opts.no_implicit_base && !opts.import_dir.is_empty() {
        let Some(base) = find_file(&opts.base_library, &cwd, opts) else {
            return Err(missing(&opts.base_library, err));
        };
        if let Some(code) = unreadable(&base, err) {
            return Err(code);
        }
    }
    let mut roots = Vec::with_capacity(opts.files.len());
    for file in &opts.files {
        let Some(found) = find_file(file, &cwd, opts) else { return Err(missing(file, err)) };
        if let Some(code) = unreadable(&found, err) {
            return Err(code);
        }
        roots.push(db.file_id(kg_base::Utf8Path::new(&found.canonical)));
    }
    Ok((db, roots))
}

/// The parse stage's errors, printed as `exitErrors` prints them
/// (`Parser/IO.hs:113-114`); `true` when there were any.
fn parse_errors(db: &kg_db::Db, roots: &[kg_base::FileId], out: &mut dyn Write) -> bool {
    let errors: Vec<Diagnostic> = db.parse_stage(roots).into_iter().filter(Diagnostic::is_error).collect();
    if !errors.is_empty() {
        let _ = write!(out, "{}", render_program_bundle(db, &errors, Layout::Errors));
    }
    !errors.is_empty()
}

/// `renderDeps` (`Main.hs:130-133`): every file the parse read, one per
/// line, sorted, less the synthetic modules (`.cmdargs.k`, `.options.k`).
/// The reference's names are `canonicalizePath`'s (`Parser/IO.hs:104`), so
/// ours are put through it again; two spellings of one file are one name.
pub fn render_deps(db: &kg_db::Db, roots: &[kg_base::FileId]) -> String {
    let program = db.program(roots);
    let mut names: Vec<String> = program
        .files
        .iter()
        .filter_map(|m| m.file())
        .filter_map(|f| db.path(f))
        .map(|p| super::options::canonical_path(p.as_str()))
        .filter(|p| !p.starts_with('.'))
        .collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| format!("{n}\n")).collect()
}

/// `writeIfChanged` (`Main.hs:135-144`): the file left alone — contents and
/// modification time — when it already holds `content`.
pub fn write_if_changed(path: &str, content: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.is_file()) && std::fs::read(path)? == content.as_bytes() {
        return Ok(());
    }
    std::fs::write(path, content)
}

/// [`write_if_changed`], a failure reported as GHC reports the `IOException`
/// nothing catches: on stderr, exit 1.
fn write_file_list(path: &str, content: &str, prog: &str, err: &mut dyn Write) -> Result<(), i32> {
    write_if_changed(path, content).map_err(|e| {
        let _ = writeln!(err, "{prog}: {path}: {e}");
        1
    })
}

/// `list-deps` (`Main.hs:111-125`): parse and import resolution, no
/// frontend; the file list to stdout, or into `--file-list`.
fn list_deps(opts: &LegacyOptions, prog: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (db, roots) = match load(opts, prog, err) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    if parse_errors(&db, &roots, out) {
        return 1;
    }
    let deps = render_deps(&db, &roots);
    if opts.file_list.is_empty() {
        let _ = write!(out, "{deps}");
    } else if let Err(code) = write_file_list(&opts.file_list, &deps, prog, err) {
        return code;
    }
    0
}

/// `pretty` (`Main.hs:44-56`) parses, prints the parse's errors as the
/// reference does, and then refuses: the reference's pretty printer is not
/// ported, by decision — `kg fmt` is this project's formatter
/// (`docs/known-divergences.md` #37). On stderr, since what `pretty` writes
/// to stdout is the program, and a refusal there would read as one.
fn pretty(opts: &LegacyOptions, prog: &str, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (db, roots) = match load(opts, prog, err) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    if parse_errors(&db, &roots, out) {
        return 1;
    }
    let _ = writeln!(
        err,
        "{prog}: `pretty` is not implemented, by decision: the reference's pretty printer is not ported, and \
         `kg fmt` is this project's formatter (docs/known-divergences.md #37); nothing was printed"
    );
    1
}

/// Runs the frontend over every source file, then the backend when it is
/// on this machine. `Ok((parsed, backend_ran))`; `Err` carries the exit
/// code.
///
/// The command-line files are the program's **roots**, exactly as
/// `Main.hs:70,76` merges them into one program: `Db::check` follows their
/// imports and the implicit base, so a diagnostic can belong to a file that was
/// never named on the command line, and is rendered against that file's own
/// source.
fn compile(
    opts: &LegacyOptions,
    cmd_args: &str,
    prog: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(usize, bool), i32> {
    let (db, roots) = load(opts, prog, err)?;
    let parsed = roots.len();

    // The manifest is refreshed as soon as the parse succeeds, so it is
    // current even when the frontend or the backend then fails
    // (`Main.hs:76-79`).
    if !opts.file_list.is_empty() && !db.parse_stage(&roots).iter().any(Diagnostic::is_error) {
        write_file_list(&opts.file_list, &render_deps(&db, &roots), prog, err)?;
    }

    let diagnostics = db.check(&roots);
    let errors: Vec<Diagnostic> = diagnostics.iter().filter(|d| d.is_error()).cloned().collect();
    if !errors.is_empty() {
        // `exitErrors` concatenates every block under one `Error 1:` prefix
        // (`Parser/IO.hs:113-117`), so the whole program's diagnostics form one
        // bundle however many files they came from. The warnings are not in
        // it: they are printed by `codegen` alone (`Main.hs:84-97`), which a
        // failed frontend never reaches (`imports.k::26`).
        let _ = write!(out, "{}", render_program_bundle(&db, &errors, Layout::Errors));
        return Err(1);
    }
    if !diagnostics.is_empty() {
        // `mapM_ (putStrLn . errorBundlePretty) ws`; `Db::check` has
        // already applied the warning set. `--Werror` fails the run after
        // them, with its own line (`Main.hs:96-99`,
        // `all-warnings-as-errors.k::1`).
        let _ = write!(out, "{}", render_program_bundle(&db, &diagnostics, Layout::Warnings));
        if opts.warnings_as_errors {
            let _ = writeln!(out, "Error 1: Warnings treated as errors");
            return Err(1);
        }
    }

    // The backend's own option validation, which would run next
    // (`config.cpp:162-207`): message to stdout, no `Error N:` prefix, exit 1.
    if let Err(message) = validate::validate(opts) {
        let _ = writeln!(out, "{message}");
        return Err(1);
    }

    // `compile` (`ParseTree.hs:31-41`): the walk into the backend, when the
    // backend is here. Without it the run is frontend-only, as it was.
    let Some(library) = kg_backend_sys::locate::backend_library() else {
        // Named and missing is a failure, never a frontend-only exit 0: the
        // gates name the library they grade against (sprint W0.4), and a
        // worktree without the compiled-in fallback once passed that way.
        if let Some(dir) = kg_backend_sys::locate::named_dir() {
            eprintln!(
                "kanagawa: no backend library at {} (KG_BACKEND_DIR)",
                dir.join(kg_backend_sys::locate::LIBRARY_FILE).display()
            );
            return Err(1);
        }
        return Ok((parsed, false));
    };
    let options = super::options::option_values(opts, cmd_args);
    let file_names: Vec<String> =
        db.program(&roots).files.iter().map(|&m| kg_ptbridge::program::file_name(&db, m)).collect();
    let codegen = kg_ptbridge::program::CodegenArgs {
        target: opts.backend.show().to_owned(),
        output: kg_ptbridge::program::output_base_name(&opts.output, &file_names),
        resource_usage: opts.resource_usage.clone(),
        dgml: opts.dgml.clone(),
        dgml_detailed: opts.dgml_detailed.clone(),
    };
    // The backend, in its worker, writes to this process's stdout itself;
    // `out` is flushed first so the frontend's text precedes it.
    let _ = out.flush();
    let outcome = kg_ptbridge::isolate::compile(&db, &roots, options, &codegen, &library);
    if let Some(d) = outcome.declined {
        // A program the reference compiles and this walk cannot: loud, and
        // never exit 0, so upstream's runner counts it.
        eprintln!(
            "kanagawa: cannot lower this program yet: {} at {} (cargo xtask ptdiff counts these)",
            d.reason,
            located(&db, d.at.head)
        );
        return Err(1);
    }
    match outcome.verdict {
        Some(Verdict::Compiled) | None => Ok((parsed, true)),
        // Each has printed its own reason, as the in-process backend did.
        Some(Verdict::Rejected | Verdict::InitFailed | Verdict::LoadFailed) => Err(1),
        Some(Verdict::Crashed { how }) => {
            eprintln!(
                "kanagawa: the backend crashed ({how}); the program was neither compiled nor rejected. \
                 This is a compiler bug: `kg build --backend trace` writes the calls it was given."
            );
            Err(BACKEND_CRASHED)
        }
    }
}

/// `path:line:col` of a range's start, one-based, where the file has a path
/// — else the range as it is, which is what a synthetic twin's has.
fn located(db: &kg_db::Db, at: kg_base::FileRange) -> String {
    let Some(path) = db.path(at.file) else { return format!("{at:?}") };
    let text = db.parse(at.file).syntax_node().text().to_string();
    let offset = usize::from(at.range.start()).min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rfind('\n').map_or(offset, |nl| offset - nl - 1) + 1;
    format!("{path}:{line}:{col}")
}

/// [`render_bundle`] over a whole program, looking each diagnostic's source up
/// by the file it belongs to.
///
/// A diagnostic with no readable source — the two synthetic modules, or a load
/// error hung on `FileId::SYNTHETIC` — still has to appear, so it falls back to
/// an empty view rather than being dropped.
fn render_program_bundle(db: &kg_db::Db, diagnostics: &[Diagnostic], layout: Layout) -> String {
    // `makeErrorBundles` (`Error.hs:168-169`): adjacent diagnostics of one
    // file are one bundle, ordered by offset within it.
    let mut bundles: Vec<Vec<&Diagnostic>> = Vec::new();
    for diagnostic in diagnostics {
        match bundles.last_mut() {
            Some(bundle) if bundle[0].range.file == diagnostic.range.file => bundle.push(diagnostic),
            _ => bundles.push(vec![diagnostic]),
        }
    }
    let mut out = String::new();
    let mut first = true;
    for bundle in &mut bundles {
        bundle.sort_by_key(|d| d.range.range.start());
        for (i, diagnostic) in bundle.iter().enumerate() {
            let file = diagnostic.range.file;
            let path = db.path(file).map(|p| p.to_string()).unwrap_or_else(|| "<generated>".to_owned());
            let text = db.vfs().read(file).map(|t| t.text).unwrap_or_default();
            let line_index = LineIndex::new(&text);
            let view = SourceView { name: &path, text: &text, line_index: &line_index };
            // A `while compiling` note may point into another module of the
            // program — the instantiation site — which the database can open.
            let locate = |range: kg_base::FileRange| {
                let name = db.path(range.file)?.to_string();
                let text = db.vfs().read(range.file)?.text;
                Some(Located { name, text })
            };
            let block = render_legacy_located(diagnostic, &view, &locate);
            // `errorBundlePretty` puts a blank line before every error of a
            // bundle but its first (megaparsec's `"\n" <> sourcePosPretty …`,
            // the first dropped) — and none between two bundles.
            if i > 0 {
                out.push('\n');
            }
            if first {
                out.push_str(&block);
                first = false;
            } else {
                let prefix = format!("Error {}: ", diagnostic.code.number());
                out.push_str(block.strip_prefix(&prefix).unwrap_or(&block));
            }
        }
        // `mapM_ (putStrLn . errorBundlePretty) ws` (`Main.hs:91`): each
        // warning bundle is its own line.
        if layout == Layout::Warnings {
            out.push('\n');
        }
    }
    // `exitError`'s `putStrLn` over the whole of `exitErrors`' string
    // (`Parser/IO.hs:113-119`).
    if layout == Layout::Errors && !out.is_empty() {
        out.push('\n');
    }
    out
}

/// How a list of diagnostics reaches stdout: `exitErrors`' one string, or
/// `codegen`'s one `putStrLn` per warning bundle.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Layout {
    Errors,
    Warnings,
}

/// What happens once the frontend has finished and found nothing.
///
/// # Why a clean frontend-only run exits 0
///
/// `--backend` defaults to `None`, whose own help text is "produce metadata
/// only" (`app/Options/CmdArgs.hs:75`). That is not a degraded mode — it is what
/// `test/syntax/syntax-test.py:103-106` invokes, so all 2,516 assertions in
/// upstream's live ctest suite are frontend-only runs that expect exit 0.
///
/// Exiting 1 there would have been the safe-looking choice and is the wrong one.
/// It makes the compat binary unusable as a `kanagawa::exe` replacement, which
/// costs the single strongest check available on this whole port: pointing
/// upstream's own runner at this binary and watching `ctest -R '^syntax\.'`.
/// A scoreboard written *from* `syntax-test.py` cannot check itself against it.
///
/// What is still withheld is the metadata file, and `--verbose` says so. A
/// request for a real backend still refuses outright — emitting no RTL and
/// claiming success is the failure this care is about.
fn not_implemented(
    opts: &LegacyOptions,
    parsed: usize,
    cmd_args: &str,
    out: &mut dyn Write,
) -> i32 {
    let plural = if parsed == 1 { "" } else { "s" };
    if opts.backend == super::options::Backend::None {
        if opts.verbosity == Verbosity::Loud {
            let _ = writeln!(
                out,
                "kanagawa: {parsed} source file{plural} passed the frontend"
            );
            let _ = writeln!(
                out,
                "kanagawa: --backend was not given, so no metadata was written; this build \
                 produces no output artifacts"
            );
        }
        return 0;
    }
    let _ = writeln!(out, "kanagawa: parsed {parsed} source file{plural}, no syntax errors");
    let _ = writeln!(
        out,
        "kanagawa: --backend={} requested; the SystemVerilog backend is not part of this build",
        opts.backend.show().to_lowercase()
    );
    let _ = writeln!(
        out,
        "kanagawa: no output was written; exiting 1 rather than reporting a compilation that did not happen"
    );
    if opts.verbosity == Verbosity::Loud {
        let _ = writeln!(out, "kanagawa: recorded command line: {cmd_args}");
    }
    1
}

/// `findFile (cwd:importDirs) file` (`Parser/IO.hs:80-82,103`): an absolute or
/// CWD-relative path first, then each import directory in search order — and
/// the file is known by `canonicalizePath` of what was found (`:104`), which
/// is what makes `kanagawa system.k` in `library/base` one module with the
/// `base.system` that `base.k` imports rather than a second copy of it (the
/// backend's "Duplicate function", 2026-09-19).
fn find_file(file: &str, cwd: &str, opts: &LegacyOptions) -> Option<Found> {
    // `findFile` asks `doesFileExist`, which a directory is not.
    let found = |joined: String| {
        std::path::Path::new(&joined)
            .is_file()
            .then(|| Found { canonical: super::options::canonical_path(&joined), joined })
    };
    found(super::options::combine(cwd, file))
        .or_else(|| opts.import_dirs().into_iter().find_map(|dir| found(super::options::combine(&dir, file))))
}

/// A command-line file [`find_file`] found.
struct Found {
    /// `path'`, as the search joined it — what a read failure names
    /// (`Parser/IO.hs:107`).
    joined: String,
    /// `canonicalizePath path'` — what the file is known by (`:104`).
    canonical: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::{FileRange, TextRange, TextSize};

    fn run_args(list: &[&str]) -> (i32, String, String) {
        let argv: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&argv, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    /// A root named by another route than its import — a symlinked
    /// directory here; a CWD-relative `system.k` in `library/base` in the
    /// wild — is the file the import finds: both are canonical
    /// (`Parser/IO.hs:104`), so the loader interns one module, not two (the
    /// backend's "Duplicate function: @base@system::cycles", 2026-09-19).
    /// Red with `find_file` returning the path as spelled.
    #[test]
    fn a_root_is_the_file_its_import_finds() {
        let dir = std::env::temp_dir().join(format!("kg-legacy-canonical-{}", std::process::id()));
        let lib = dir.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("m.k"), "module m { F }\n\ninline void F()\n{\n}\n").unwrap();
        let link = dir.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&lib, &link).unwrap();
        let opts = LegacyOptions { import_dir: vec![lib.to_str().unwrap().to_owned()], ..LegacyOptions::default() };
        let settings = super::super::options::settings_from(&opts, dir.to_str().unwrap());
        let root = find_file(link.join("m.k").to_str().unwrap(), dir.to_str().unwrap(), &opts).unwrap().canonical;
        let imported = settings.import_dirs[0].join("m.k");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(root, imported.as_str());
    }

    /// `handle`'s two tails (`Main.hs:84-99`): a failed frontend prints its
    /// error bundle and nothing else — the warnings are `codegen`'s, which
    /// it never reaches (`imports.k::26`); a clean one prints its warnings,
    /// and `--Werror` then fails the run with its own line
    /// (`all-warnings-as-errors.k::1`).
    #[test]
    fn warnings_print_only_without_errors_and_werror_says_so() {
        let dir = std::env::temp_dir().join(format!("kg-legacy-warnings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.k");
        std::fs::write(&base, "module base\n    {\n    }\n").unwrap();
        let narrowing = "inline void Foo(uint32 x)\n{\n}\n\ninline void main()\n{\n    int32 x = 0;\n    Foo(x);\n}\n";
        let warned = dir.join("warned.k");
        std::fs::write(&warned, narrowing).unwrap();
        // An unused import is a warning the parser raises, so it stands
        // beside any error the passes find.
        std::fs::write(dir.join("m.k"), "module m\n    { F\n    }\n\ninline void F()\n{\n}\n").unwrap();
        let failed = dir.join("failed.k");
        std::fs::write(&failed, "import m\n\ninline void g()\n{\n    Go();\n}\n").unwrap();
        let base_arg = format!("--base-library={}", base.to_str().unwrap());
        let import_arg = format!("--import-dir={}", dir.to_str().unwrap());
        // Both runs stop before the backend, which a unit test cannot host.
        let run_on = |file: &std::path::Path| {
            run_args(&["kanagawa", base_arg.as_str(), import_arg.as_str(), "--Wall", "--Werror", file.to_str().unwrap()])
        };
        let (werror_code, werror_out, _) = run_on(&warned);
        let (failed_code, failed_out, _) = run_on(&failed);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(werror_code, 1, "{werror_out}");
        assert!(werror_out.contains("warning: Narrowing conversion"), "{werror_out}");
        assert!(werror_out.ends_with("\nError 1: Warnings treated as errors\n"), "{werror_out}");
        assert_eq!(failed_code, 1, "{failed_out}");
        assert!(failed_out.starts_with("Error 1: "), "{failed_out}");
        assert!(!failed_out.contains("warning:"), "{failed_out}");
    }

    /// `--verbose` is cmdargs' `Loud`, which the reference's `Main` reads
    /// nowhere: its stdout under `-v` is the backend's own (`Device name:`
    /// …), not a line of the frontend's (the reference binary,
    /// 2026-10-10). Red with the port's former "command line recorded"
    /// line.
    #[test]
    fn verbose_adds_no_line_of_the_frontend_s() {
        let (_, out, _) = run_args(&["kanagawa", "--verbose", "definitely-not-here.k"]);
        assert!(!out.contains("kanagawa:"), "{out}");
    }

    #[test]
    fn no_arguments_prints_help_to_stdout_and_exits_zero() {
        let (code, out, err) = run_args(&["kanagawa"]);
        assert_eq!(code, 0);
        assert!(out.contains("Kanagawa Compiler"), "{out}");
        assert!(err.is_empty(), "{err}");
    }

    #[test]
    fn flags_but_no_files_is_the_reference_message_on_stdout() {
        let (code, out, err) = run_args(&["kanagawa", "--backend=sv"]);
        assert_eq!(code, 1);
        assert_eq!(out, "Missing source filename(s)\n");
        assert!(err.is_empty(), "{err}");
    }

    #[test]
    fn an_unparseable_command_line_goes_to_stderr() {
        let (code, out, err) = run_args(&["kanagawa", "--no-such-flag", "a.k"]);
        assert_eq!(code, 1);
        assert!(out.is_empty(), "{out}");
        assert!(err.contains("Unknown flag: --no-such-flag"), "{err}");
    }

    /// A command-line file not found is the exception `parseFile` throws
    /// (`Parser/IO.hs:83`), which nothing catches: GHC's handler prints it
    /// on stderr after the program's name, the message `importDirs` and
    /// all (`:108,110`) — not an `Error 1:` diagnostic on stdout, as this
    /// test said until 2026-10-09 (the reference, probed: `kanagawa
    /// --import-dir=/tmp/a nothere.k` prints nothing on stdout).
    #[test]
    fn a_missing_source_file_is_an_exception_on_stderr() {
        let (code, out, err) =
            run_args(&["/bin/kanagawa", "--no-implicit-base", "--import-dir=/nowhere/", "definitely-not-here.k"]);
        assert_eq!(code, 1);
        assert_eq!(out, "", "{out}");
        assert_eq!(
            err,
            "kanagawa: Can't find file: definitely-not-here.k in any of the import directories\n\
             /nowhere/device/mock\n/nowhere/\n\n"
        );
    }

    /// A command-line file that is not UTF-8 is `readFileUtf8`'s exception,
    /// uncaught the same way (`Parser/IO.hs:83,106-107`), naming the path as
    /// found rather than canonicalised — on macOS the temporary directory is
    /// a symlink, so the two differ here. The reference, probed 2026-10-10
    /// on `test/syntax/imports/binary.k`'s four bytes: this line on stderr,
    /// nothing on stdout, exit 1. Until then this compiled clean.
    #[test]
    fn a_source_file_that_is_not_utf8_is_an_exception_on_stderr() {
        let dir = std::env::temp_dir().join(format!("kg-cli-binary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("binary.k");
        std::fs::write(&path, b"\xc0\xde\xde\xad").unwrap();
        let path = path.to_str().unwrap().to_owned();

        let (code, out, err) = run_args(&["kanagawa", "--no-implicit-base", &path]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((code, out.as_str()), (1, ""));
        assert_eq!(
            err,
            format!("kanagawa: {path}: hGetContents: invalid argument (cannot decode byte sequence starting from 192)\n")
        );
    }

    /// The base library is looked for first (`importBase`,
    /// `Parser/IO.hs:72-77`), so a missing one is the same exception, ahead
    /// of every source file; with no `--import-dir`, `.cmdargs` stands in for
    /// it and nothing is looked for.
    #[test]
    fn a_missing_base_library_is_the_first_exception() {
        let (code, out, err) = run_args(&["kanagawa", "--import-dir=/nowhere", "also-not-here.k"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.starts_with("kanagawa: Can't find file: base.k in any of the import directories\n"), "{err}");
        let (_, _, err) = run_args(&["kanagawa", "also-not-here.k"]);
        assert!(err.starts_with("kanagawa: Can't find file: also-not-here.k"), "{err}");
    }

    /// `exitErrors` emits the prefix **once**, ahead of the concatenated blocks
    /// (`Parser/IO.hs:113-117`). `test/syntax/syntax-test.py` matches the first
    /// `Error (\d+)` and would read a second prefix as a second compilation.
    ///
    /// Driven through `render_program_bundle`, the function the binary actually
    /// calls, and across **two files** — which is the case the whole-program
    /// door introduced and the old single-view renderer could not express.
    #[test]
    fn the_error_one_prefix_appears_exactly_once_per_bundle() {
        let mut db = kg_db::Db::new(kg_config::Settings::fallback("/proj"));
        let a = db.set_file_text(
            kg_base::Utf8Path::new("/proj/a.k"),
            "inline void f()\n{\n    uint32 x = ;\n}\n".to_owned(),
        );
        let b = db.set_file_text(
            kg_base::Utf8Path::new("/proj/b.k"),
            "inline void g()\n{\n}\n".to_owned(),
        );
        let at = |file, offset: u32| {
            FileRange::new(file, TextRange::empty(TextSize::from(offset)))
        };
        let diagnostics = vec![
            Diagnostic::error(at(a, 31), "first"),
            Diagnostic::error(at(b, 5), "second"),
        ];
        let rendered = render_program_bundle(&db, &diagnostics, Layout::Errors);
        assert_eq!(rendered.matches("Error 1: ").count(), 1, "{rendered}");
        assert!(rendered.contains("first") && rendered.contains("second"), "{rendered}");
        // Each block is rendered against its own file, not the first one.
        assert!(rendered.contains("a.k") && rendered.contains("b.k"), "{rendered}");
    }

    /// The reference's layout, byte for byte (the reference binary,
    /// 2026-10-10): within one file's bundle a blank line before every
    /// diagnostic but the first (`errorBundlePretty`); none between two
    /// files' bundles (`concatMap`); `exitError`'s `putStrLn` after the
    /// whole, and one `putStrLn` per warning bundle (`Main.hs:91`). Red with
    /// the blank lines dropped, as they were (divergence #38).
    #[test]
    fn diagnostics_are_laid_out_as_megaparsec_bundles() {
        let mut db = kg_db::Db::new(kg_config::Settings::fallback("/proj"));
        let text = "inline void f()\n{\n    uint32 x = y;\n    uint32 z = w;\n}\n";
        let a = db.set_file_text(kg_base::Utf8Path::new("/proj/a.k"), text.to_owned());
        let b = db.set_file_text(kg_base::Utf8Path::new("/proj/b.k"), text.to_owned());
        let at = |file, offset: u32| FileRange::new(file, TextRange::empty(TextSize::from(offset)));
        let y = text.find(" y;").unwrap() as u32 + 1;
        let w = text.find(" w;").unwrap() as u32 + 1;
        // Out of order within `a`: a bundle is sorted by offset.
        let errors = vec![
            Diagnostic::error(at(a, w), "second"),
            Diagnostic::error(at(a, y), "first"),
            Diagnostic::error(at(b, y), "third"),
        ];
        let rendered = render_program_bundle(&db, &errors, Layout::Errors);
        let expected = "Error 1: /proj/a.k:3:16:\n  |\n3 |     uint32 x = y;\n  |                ^\nfirst\n\n\
                        /proj/a.k:4:16:\n  |\n4 |     uint32 z = w;\n  |                ^\nsecond\n\
                        /proj/b.k:3:16:\n  |\n3 |     uint32 x = y;\n  |                ^\nthird\n\n";
        assert_eq!(rendered, expected);
        // One bundle of two warnings: a blank line between, and its own
        // `putStrLn`'s after.
        let kind = kg_diag::WarningKind::Conversion;
        let warnings = vec![Diagnostic::warning(at(a, y), kind, "careful"), Diagnostic::warning(at(a, w), kind, "again")];
        let rendered = render_program_bundle(&db, &warnings, Layout::Warnings);
        assert!(rendered.starts_with("/proj/a.k:3:16:\n"), "{rendered:?}");
        assert!(rendered.contains("\n\n/proj/a.k:4:16:\n"), "{rendered:?}");
        assert!(rendered.ends_with("\n\n") && !rendered.ends_with("\n\n\n"), "{rendered:?}");
        assert_eq!(rendered.matches("\n\n").count(), 2, "{rendered:?}");
    }

    /// A scratch program: `base.k`, a module `zmod` and a root `a_root.k`
    /// importing it, under a directory that is its own import directory —
    /// named so that sorted order (`a_root`, `base`, `zmod`) is not the
    /// program's (`base`, `zmod`, `a_root`).
    struct Program {
        dir: std::path::PathBuf,
    }

    impl Program {
        fn new(name: &str, root: &str) -> Program {
            let dir = std::env::temp_dir().join(format!("kg-legacy-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("base.k"), "module base\n    {\n    }\n").unwrap();
            std::fs::write(dir.join("zmod.k"), "module zmod\n    { F\n    }\n\ninline void F()\n{\n}\n").unwrap();
            std::fs::write(dir.join("a_root.k"), root).unwrap();
            // Canonical, as the paths the compiler prints are (`/tmp` is a
            // link on macOS).
            Program { dir: std::fs::canonicalize(&dir).unwrap() }
        }

        fn path(&self, name: &str) -> String {
            self.dir.join(name).to_str().unwrap().to_owned()
        }

        /// `kanagawa <args…> --base-library=… --import-dir=… a_root.k`.
        fn run(&self, args: &[&str]) -> (i32, String, String) {
            let base = format!("--base-library={}", self.path("base.k"));
            let import = format!("--import-dir={}", self.dir.to_str().unwrap());
            let root = self.path("a_root.k");
            let mut argv = vec!["kanagawa"];
            argv.extend(args);
            argv.extend([base.as_str(), import.as_str(), root.as_str()]);
            run_args(&argv)
        }
    }

    impl Drop for Program {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const IMPORTS_ZMOD: &str = "import zmod\n\ninline void main()\n{\n    F();\n}\n";

    /// `list-deps` prints every file the parse read, sorted (`renderDeps`,
    /// `Main.hs:130-133`), and nothing else. Red with the sort removed.
    #[test]
    fn list_deps_prints_every_file_the_parse_read_sorted() {
        let p = Program::new("list-deps", IMPORTS_ZMOD);
        let (code, out, err) = p.run(&["list-deps"]);
        assert_eq!((code, err.as_str()), (0, ""), "{out}");
        assert_eq!(out, format!("{}\n{}\n{}\n", p.path("a_root.k"), p.path("base.k"), p.path("zmod.k")));
    }

    /// A parse error is `exitErrors`, as `compile` prints it, and no list
    /// (`Main.hs:116-117`); a frontend error is not seen, since the
    /// frontend never runs (`Main.hs:112`).
    #[test]
    fn list_deps_stops_at_a_parse_error_and_never_runs_the_frontend() {
        let broken = Program::new("list-deps-broken", "import zmod\n\ninline void main(\n");
        let (code, out, _) = broken.run(&["list-deps"]);
        assert_eq!(code, 1, "{out}");
        assert!(out.starts_with("Error 1: "), "{out}");
        let wrong = Program::new("list-deps-wrong", "import zmod\n\ninline void main(uint8 a)\n{\n    bool b = !a;\n}\n");
        let (code, out, _) = wrong.run(&["list-deps"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.ends_with("zmod.k\n"), "{out}");
    }

    /// `--file-list` takes the list instead of stdout, and an unchanged
    /// list leaves the file — and its modification time — alone
    /// (`writeIfChanged`, `Main.hs:135-144`). Red with the comparison
    /// dropped.
    #[test]
    fn the_file_list_is_written_only_when_it_changes() {
        let p = Program::new("file-list", IMPORTS_ZMOD);
        let list = p.path("deps.txt");
        let flag = format!("--file-list={list}");
        let (code, out, err) = p.run(&["list-deps", &flag]);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
        let written = std::fs::read_to_string(&list).unwrap();
        assert!(written.starts_with(&p.path("a_root.k")), "{written}");
        let past = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options().write(true).open(&list).unwrap().set_modified(past).unwrap();
        let (code, _, _) = p.run(&["list-deps", &flag]);
        assert_eq!(code, 0);
        assert_eq!(std::fs::metadata(&list).unwrap().modified().unwrap(), past, "an unchanged list was rewritten");
        std::fs::write(&list, "stale\n").unwrap();
        let (code, _, _) = p.run(&["list-deps", &flag]);
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(&list).unwrap(), written);
    }

    /// `compile --file-list` writes the same list as soon as the parse
    /// succeeds — so a frontend error still leaves it written — and not at
    /// all when the parse fails (`Main.hs:73-79`). Red with the write moved
    /// after the frontend.
    #[test]
    fn compile_writes_the_file_list_before_the_frontend_can_fail() {
        let wrong = Program::new("compile-list", "import zmod\n\ninline void main(uint8 a)\n{\n    bool b = !a;\n}\n");
        let list = wrong.path("deps.txt");
        let flag = format!("--file-list={list}");
        let (code, out, _) = wrong.run(&[&flag]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("Expected a boolean operand"), "{out}");
        let (_, listed, _) = wrong.run(&["list-deps"]);
        assert_eq!(std::fs::read_to_string(&list).unwrap(), listed);

        let broken = Program::new("compile-list-broken", "import zmod\n\ninline void main(\n");
        let list = broken.path("deps.txt");
        let (code, _, _) = broken.run(&[&format!("--file-list={list}")]);
        assert_eq!(code, 1);
        assert!(!std::path::Path::new(&list).exists(), "a failed parse wrote the list");
    }

    /// `--template-iterations=0` is the pass list once and no instantiation
    /// (`Template.hs:57`): a call that needs one is left for pass 12, which
    /// fails it — the reference's own message for this program, probed
    /// 2026-10-09. Red with the budget left at the fixpoint.
    #[test]
    fn template_iterations_bound_the_instantiation_loop() {
        let p = Program::new(
            "template-iterations",
            "template <typename T>\ninline T id(T x) { return x; }\n\ninline uint32 main(uint32 a)\n{\n    return id(a);\n}\n",
        );
        let (code, out, _) = p.run(&["--template-iterations=0"]);
        assert_eq!(code, 1, "{out}");
        assert!(out.starts_with("Error 1: ") && out.contains("Failed to instantiate template"), "{out}");
        let opts = |n: i64| LegacyOptions { template_iterations: n, ..LegacyOptions::default() };
        assert_eq!(pass_budget(&opts(0)), kg_db::PassBudget::AtMost(0));
        assert_eq!(pass_budget(&opts(3)), kg_db::PassBudget::AtMost(3));
        assert_eq!(pass_budget(&opts(i64::MAX)), kg_db::PassBudget::Fixpoint);
        assert_eq!(pass_budget(&opts(-1)), kg_db::PassBudget::Fixpoint, "a negative count never reaches 0");
    }

    /// A pass bound this compiler cannot stop at is refused, by name, on
    /// stderr — and only one that would bound something: all nineteen
    /// passes, or a `--template-passes` with no iteration count, are the
    /// default run (`Frontend.hs:38`, `Template.hs:62-64`).
    #[test]
    fn unreachable_pass_bounds_are_refused_and_reachable_ones_run() {
        let opts = |passes: i64, iterations: i64, template_passes: i64| LegacyOptions {
            passes,
            template_iterations: iterations,
            template_passes,
            ..LegacyOptions::default()
        };
        assert!(unhonoured_pass_bounds(&opts(18, i64::MAX, i64::MAX)).is_some());
        assert!(unhonoured_pass_bounds(&opts(19, i64::MAX, i64::MAX)).is_none());
        assert!(unhonoured_pass_bounds(&opts(i64::MAX, i64::MAX, 3)).is_none());
        assert!(unhonoured_pass_bounds(&opts(i64::MAX, 1, 8)).is_some());
        assert!(unhonoured_pass_bounds(&opts(i64::MAX, 1, 9)).is_none());
        let (code, out, err) = run_args(&["kanagawa", "--frontend-passes=5", "a.k"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.starts_with("kanagawa: --frontend-passes=5 is not supported by this build"), "{err}");
    }

    /// The `--dump-*` flags that print the reference's own tree say so on
    /// stderr, and the compile goes on (`docs/known-divergences.md` #36).
    #[test]
    fn unimplemented_dumps_are_said_on_stderr_and_the_compile_goes_on() {
        let wrong = Program::new("dumps", "import zmod\n\ninline void main(uint8 a)\n{\n    bool b = !a;\n}\n");
        let (code, out, err) = wrong.run(&["--dump-parse", "--dump-source"]);
        assert_eq!(code, 1);
        assert!(out.contains("Expected a boolean operand"), "{out}");
        assert!(err.contains("--dump-parse is not implemented"), "{err}");
        assert!(err.contains("--dump-source is not implemented, by decision"), "{err}");
        assert!(!err.contains("--dump-types"), "{err}");
    }

    /// `pretty` prints the parse's errors as the reference does, and
    /// otherwise refuses on stderr — never the source, never on stdout.
    #[test]
    fn pretty_parses_then_refuses_on_stderr() {
        let p = Program::new("pretty", IMPORTS_ZMOD);
        let (code, out, err) = p.run(&["pretty"]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("`kg fmt` is this project's formatter"), "{err}");
        let broken = Program::new("pretty-broken", "inline void main(\n");
        let (code, out, _) = broken.run(&["pretty"]);
        assert_eq!(code, 1);
        assert!(out.starts_with("Error 1: "), "{out}");
    }
}
