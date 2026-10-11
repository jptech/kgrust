//! `kg` — the modern front door.
//!
//! Seven commands: `check`, `fmt`, `dump`, `explain`, `lsp`, `doc` (alias
//! `docs`), and `build`, which runs the frontend and then walks the program
//! into the reference's C++ backend (`kg-ptbridge`). A program the walk cannot
//! lower yet is refused with exit 2, never compiled wrong.
//!
//! Exit codes:
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | success |
//! | 1 | the command ran and found errors |
//! | 2 | a usage error, or a program `build` cannot lower yet |
//! | 3 | `build`: the backend crashed — no verdict either way |

pub mod dump;
pub mod render;
pub mod settings;
pub mod style;

use std::io::Write;

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Parser, Subcommand};
use kg_base::FileId;

use crate::explain;
use render::{ErrorFormat, Source};
use settings::{Env, Overrides, Resolved};
use style::{ColorChoice, Styles};

const ABOUT: &str = "The Kanagawa toolchain: check, build, format, document, explain and inspect Kanagawa sources.";

const LONG_ABOUT: &str = "\
The Kanagawa toolchain.

What works today: `check` (parse, resolve, fold and type-check, following
imports), `lsp` (the language server, over stdio), `fmt` (the comment-preserving
formatter), `dump` (token stream, CST, AST, IR, instances), `explain` (error and warning
reference), `doc` (a static API-documentation site from the doc comments) and
`build` (the frontend, then the reference's C++ backend through
its ParseTree ABI — `libkanagawa-backend`, found via `KG_BACKEND_DIR`; a
program the walk cannot yet lower is refused with exit 2, never compiled
wrong).

`check` runs the whole frontend — every pass of the reference compiler's,
graded against it assertion by assertion on its own test corpus — but not the
backend: what the reference's C++ backend rejects (its `Error N` codes past the
frontend) is reported by `build` alone, so a clean `check` is not a guarantee
that the reference compiler accepts the program. A reported error is one the
reference reports too; the deliberate differences are listed in the project's
docs/known-divergences.md.

For the legacy command line — every flag of the reference compiler, byte-for-byte
argv compatible — use the `kanagawa` binary from this same package.

Exit codes: 0 success, 1 errors found, 2 bad usage, or not implemented yet (a
program `build` cannot lower), 3 the backend crashed during `build` (it runs in
a worker process, so the crash is reported rather than taking `kg` down).";

#[derive(Parser, Debug)]
#[command(name = "kg", version, about = ABOUT, long_about = LONG_ABOUT, disable_help_subcommand = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug, Clone, Default)]
pub struct GlobalArgs {
    /// Path to `kanagawa.toml` (or the directory holding it).
    #[arg(short = 'C', long, global = true, value_name = "FILE")]
    pub manifest_path: Option<String>,
    /// Ignore `kanagawa.toml` entirely and use built-in defaults.
    #[arg(long, global = true)]
    pub no_manifest: bool,
    /// Named `[profile.<NAME>]` overlay from the manifest.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,
    /// More detail; repeatable (`-vv` prints the resolved configuration).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Print diagnostics only, no summary.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    /// When to colourise output.
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto")]
    pub color: ColorChoice,
    /// Diagnostic output shape.
    #[arg(long, global = true, value_name = "FORMAT", default_value = "human")]
    pub error_format: ErrorFormat,
    /// Worker threads. Accepted for forward compatibility; the frontend is
    /// single-threaded today.
    #[arg(short = 'j', long, global = true, value_name = "N")]
    pub jobs: Option<i64>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Parse and diagnose, writing no output artifacts.
    #[command(long_about = "\
Parse the given files (or `project.sources` from the manifest) and report every
problem found, with a source excerpt and a caret.

The named files are program roots: their imports and the implicit base library
are loaded and checked too, so a diagnostic may point at a file you did not
name.

The backend's own checks are not run here — `build` runs them — so a clean
run is not a guarantee that the reference compiler accepts the program.")]
    Check(CheckArgs),
    /// Format sources in place, or check that they are formatted.
    Fmt(FmtArgs),
    /// Run the language server over stdio.
    Lsp(LspArgs),
    /// Print the token stream, the CST, the AST, the lowered IR or the
    /// template instances the program-wide loop made of one file's program.
    Dump(DumpArgs),
    /// Explain an error code or a warning.
    Explain(ExplainArgs),
    /// Compile to RTL.
    Build(BuildArgs),
    /// Write API documentation from the sources' doc comments.
    #[command(
        alias = "docs",
        long_about = "\
Write a static site documenting the given files (or `project.sources` from
the manifest) from their doc comments — `//|` before a declaration, `//<`
after it, in Markdown — as the reference's documentation generator reads
them: a page per module, its exposed declarations with their signatures,
parameters and members, and a search over all of them.

A name in a signature links to the declaration it resolves to when that one
is documented in the same site; pass the library's directory along with your
own to link into it too.")]
    Doc(DocArgs),
}

/// The source-resolution flags shared by `check` and `build`, layered on top of
/// the manifest (`docs/spec/cli.md` §9.2).
#[derive(Args, Debug, Clone, Default)]
pub struct SourceArgs {
    /// Directory to search for imports; repeatable, **order significant**.
    #[arg(short = 'I', long = "import-dir", value_name = "DIR")]
    pub import_dir: Vec<String>,
    /// Define a global constant, `NAME=VALUE` (`#` also accepted).
    #[arg(short = 'D', long = "define", value_name = "NAME=VALUE")]
    pub define: Vec<String>,
    /// Define a global type alias, `NAME=TYPE` (`#` also accepted).
    #[arg(short = 'U', long = "using", value_name = "NAME=TYPE")]
    pub using: Vec<String>,
    /// Base library file.
    #[arg(long, value_name = "FILE")]
    pub base_library: Option<String>,
    /// Do not implicitly import the base library.
    #[arg(long)]
    pub no_implicit_base: bool,
    /// Target FPGA device; also selects `<import-dir>/device/<name>`.
    #[arg(long, value_name = "NAME")]
    pub target_device: Option<String>,
    /// Parse documentation comments.
    #[arg(long)]
    pub parse_docs: bool,
    /// Warning switch: `all`, `error`, `<name>` or `no-<name>`; repeatable,
    /// applied in order.
    #[arg(short = 'W', value_name = "SWITCH")]
    pub warn: Vec<String>,
}

impl SourceArgs {
    fn overrides(&self) -> Overrides {
        Overrides {
            import_dirs: self.import_dir.clone(),
            defines: self.define.clone(),
            using: self.using.clone(),
            base_library: self.base_library.clone(),
            no_implicit_base: self.no_implicit_base,
            target_device: self.target_device.clone(),
            parse_docs: self.parse_docs,
            warnings: self.warn.clone(),
        }
    }
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Files or directories. Defaults to `project.sources` from the manifest.
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    #[command(flatten)]
    pub source: SourceArgs,
}

#[derive(Args, Debug)]
pub struct DocArgs {
    /// Files or directories to document. Defaults to `project.sources` from the manifest.
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    /// Where to write the site. Defaults to `target/doc` beside the manifest,
    /// or in the current directory without one.
    #[arg(short = 'o', long, value_name = "DIR")]
    pub out: Option<String>,
    /// The site's title. Defaults to the manifest's package name.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    /// Open the site in a browser once written.
    #[arg(long)]
    pub open: bool,
    #[command(flatten)]
    pub source: SourceArgs,
}

#[derive(Args, Debug)]
pub struct FmtArgs {
    /// Files or directories. Defaults to `project.sources` from the manifest.
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    /// Do not write; exit 1 if any file would change.
    #[arg(long)]
    pub check: bool,
    /// Write to stdout instead of in place.
    #[arg(long, conflicts_with = "check")]
    pub stdout: bool,
    /// Override `[fmt] line-width`.
    #[arg(long, value_name = "N")]
    pub line_width: Option<usize>,
    /// Override `[fmt] indent`.
    #[arg(long, value_name = "N")]
    pub indent: Option<usize>,
    /// Override `[fmt] use-tabs`.
    #[arg(long)]
    pub use_tabs: bool,
    /// Override `[fmt] newline`.
    #[arg(long, value_name = "LF|CRLF")]
    pub newline: Option<String>,
    /// Override `[fmt] sort-imports`.
    #[arg(long)]
    pub sort_imports: bool,
}

#[derive(Args, Debug)]
pub struct LspArgs {
    /// Communicate over stdin/stdout. The only transport, accepted so that
    /// editor configurations that pass it keep working.
    #[arg(long, default_value_t = true)]
    pub stdio: bool,
}

#[derive(Args, Debug)]
pub struct DumpArgs {
    /// What to print.
    #[arg(value_name = "WHAT")]
    pub what: dump::What,
    /// The file to read; `-` reads stdin.
    #[arg(value_name = "FILE")]
    pub file: String,
    /// Omit whitespace and comments from the tree.
    #[arg(long)]
    pub no_trivia: bool,
    /// `ir` only: run pass 8's rewriting sub-passes over the lowered IR first.
    ///
    /// The reference materialises declarations nobody wrote — an abbreviated
    /// `void f(auto a)` becomes a template *and* the original
    /// (`Function.hs:288-311`) — and a lossless CST cannot carry them. This is
    /// the window onto what those rewrites make, and the human counterpart of
    /// `kg-oracle --tree --passes 8 --template-passes N`.
    #[arg(long)]
    pub rewrite: bool,
    /// Source resolution — `instances` and `ir` load the whole program with
    /// it; `cst` and `ast` read only its `--parse-docs`.
    #[command(flatten)]
    pub source: SourceArgs,
}

#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// An error code (`1`, `E27`, `Warning 12`) or a warning name (`shadow`).
    #[arg(value_name = "CODE", required_unless_present = "list")]
    pub code: Option<String>,
    /// List every known code instead.
    #[arg(long)]
    pub list: bool,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Files or directories. Defaults to `project.sources` from the manifest.
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    /// Code generation backend.
    #[arg(long, value_name = "sv|none", default_value = "sv")]
    pub backend: String,
    /// Base name of the output file(s).
    #[arg(short = 'o', long, value_name = "PREFIX")]
    pub output: Option<String>,
    /// Optimization level.
    #[arg(short = 'O', value_name = "LEVEL", default_value_t = 2)]
    pub optimize: u8,
    /// Extra artifacts: `ir`, `mlir`, `dgml=<FILE>`, `resource-usage=<FILE>`,
    /// `dep-list=<FILE>`.
    #[arg(long, value_name = "KIND")]
    pub emit: Vec<String>,
    /// Debug mode.
    #[arg(long)]
    pub debug: bool,
    /// Unstable knob, `-Z <key>=<value>`; `hw.<key>` reaches the backend table.
    #[arg(short = 'Z', value_name = "KEY=VALUE")]
    pub unstable: Vec<String>,
    #[command(flatten)]
    pub source: SourceArgs,
}

/// Runs `kg`. Returns the process exit code.
pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(error) => {
            // clap prints `--help`/`--version` through the same error channel;
            // those are a success and go to stdout.
            let text = error.render().ansi().to_string();
            if error.use_stderr() {
                let _ = write!(err, "{text}");
                return 2;
            }
            let _ = write!(out, "{text}");
            return 0;
        }
    };

    // `--color` wins when it is given; otherwise `KG_COLOR` gets a say before
    // the terminal check does. `auto` is clap's default, so it is also what an
    // absent flag looks like — which is exactly when the environment should
    // apply.
    let color = match cli.global.color {
        ColorChoice::Auto => Env::from_process()
            .color
            .as_deref()
            .and_then(ColorChoice::parse)
            .unwrap_or(ColorChoice::Auto),
        explicit => explicit,
    };
    let styles = Styles::new(color);
    match &cli.command {
        Command::Check(args) => check(&cli.global, args, &styles, out, err),
        Command::Dump(args) => dump_command(&cli.global, args, &styles, out, err),
        Command::Explain(args) => explain_command(args, out, err),
        Command::Fmt(args) => fmt(&cli.global, args, &styles, out, err),
        Command::Lsp(args) => lsp(args, err),
        Command::Build(args) => build(&cli.global, args, &styles, out, err),
        Command::Doc(args) => doc(&cli.global, args, out, err),
    }
}

/// `kg doc`: the site `kg_doc::write` makes of the inputs.
fn doc(global: &GlobalArgs, args: &DocArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(resolved) = resolve(global, &args.source, &args.paths, err) else { return 2 };
    let files = match collect_inputs(&args.paths, &resolved, err) {
        Ok(files) if files.is_empty() => {
            let _ = writeln!(err, "kg: no input files. Pass paths, or list them under `project.sources` in kanagawa.toml.");
            return 2;
        }
        Ok(files) => files,
        Err(code) => return code,
    };
    let db = kg_db::Db::new(resolved.settings.clone());
    let mut inputs = Vec::with_capacity(files.len());
    for path in &files {
        if db.vfs().read_path(path).is_none() {
            let _ = writeln!(err, "kg: cannot read {path}");
            return 2;
        }
        inputs.push((db.file_id(path), path.clone()));
    }
    let project_dir = resolved.manifest_path.as_ref().and_then(|m| m.parent().map(Utf8Path::to_owned)).unwrap_or_else(cwd);
    let site = kg_doc::Site {
        title: args
            .title
            .clone()
            .or_else(|| package_name(&resolved))
            .unwrap_or_else(|| "API documentation".to_owned()),
        out: args.out.as_ref().map_or_else(|| project_dir.join("target/doc"), Utf8PathBuf::from),
    };
    match kg_doc::write(&db, &inputs, &site) {
        Ok(written) => {
            let _ = writeln!(
                out,
                "Documented {} modules, {} declarations ({} with documentation): {}",
                written.modules, written.declarations, written.documented, written.index
            );
            if args.open {
                let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
                if let Err(e) = std::process::Command::new(opener).arg(written.index.as_str()).status() {
                    let _ = writeln!(err, "kg: could not open {}: {e}", written.index);
                }
            }
            0
        }
        Err(e) => {
            let _ = writeln!(err, "kg: writing {}: {e}", site.out);
            1
        }
    }
}

/// `[package] name` from the manifest, if there is one and it has one.
fn package_name(resolved: &Resolved) -> Option<String> {
    let text = std::fs::read_to_string(resolved.manifest_path.as_ref()?).ok()?;
    let manifest: toml::Value = toml::from_str(&text).ok()?;
    manifest.get("package")?.get("name")?.as_str().map(str::to_owned)
}

fn cwd() -> Utf8PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("."))
}

/// `paths` are the command's input files, and they are here only so that
/// manifest discovery can start beside the *file* instead of beside the shell.
/// A user who runs `kg check ../other-project/top.k` means that project's
/// manifest, which is the one their editor already reads for the same buffer.
fn resolve(
    global: &GlobalArgs,
    source: &SourceArgs,
    paths: &[String],
    err: &mut dyn Write,
) -> Option<Resolved> {
    let cwd = cwd();
    // The first input, not all of them: a manifest per file would mean a
    // command whose settings change halfway through the file list.
    let start = paths
        .first()
        .map(Utf8PathBuf::from)
        .and_then(|p| if p.is_dir() { Some(p) } else { p.parent().map(Utf8Path::to_owned) })
        .filter(|p| !p.as_str().is_empty())
        .unwrap_or_else(|| cwd.clone());
    match settings::resolve(
        global.manifest_path.as_deref(),
        global.no_manifest,
        global.profile.as_deref(),
        &Env::from_process(),
        &source.overrides(),
        &cwd,
        &start,
    ) {
        Ok(resolved) => Some(resolved),
        Err(message) => {
            let _ = writeln!(err, "kg: {message}");
            None
        }
    }
}

/// `kg check`: load the program rooted at the given files and report every
/// diagnostic the frontend produces.
///
/// The inputs are **roots**, not the unit of work. `Db::check` follows their
/// imports and the implicit base library (`Parser/IO.hs:66-77`), so a diagnostic
/// may land in a file the user never named — an error in an imported module
/// stops the build just as surely, and saying so beats reporting a phantom
/// unresolved name at the import site. Files are grouped and rendered in the
/// order the loader reached them.
fn check(
    global: &GlobalArgs,
    args: &CheckArgs,
    styles: &Styles,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(resolved) = resolve(global, &args.source, &args.paths, err) else { return 2 };

    if global.verbose >= 2 {
        let _ = write!(out, "{}", describe_settings(&resolved));
    }

    let files = match collect_inputs(&args.paths, &resolved, err) {
        Ok(files) if files.is_empty() => {
            let _ = writeln!(
                err,
                "kg: no input files. Pass paths, or list them under `project.sources` in kanagawa.toml."
            );
            return 2;
        }
        Ok(files) => files,
        Err(code) => return code,
    };

    let db = kg_db::Db::new(resolved.settings.clone());

    // Unreadable roots are reported here rather than left to the loader: the
    // loader's job is to explain a failed *import*, and "you named a file that
    // does not exist" is a different error with a better message.
    let mut unreadable = 0usize;
    let mut roots = Vec::with_capacity(files.len());
    for path in &files {
        if db.vfs().read_path(path).is_none() {
            let _ = writeln!(err, "kg: cannot read {path}");
            unreadable += 1;
            continue;
        }
        roots.push(db.file_id(path));
    }

    // `Db::check` applies `settings.warnings` itself, so every consumer of the
    // query layer — this, the language server, the conformance harness — agrees
    // on which warnings exist.
    let diagnostics = db.check(&roots);
    phases(&db, err);

    let mut errors = 0usize;
    let mut warnings = 0usize;

    // `db.check` already returns program order, so one pass suffices; the cache
    // holds only the file currently being rendered.
    let mut current: Option<(FileId, kg_base::Utf8PathBuf, kg_vfs::FileText)> = None;
    for diagnostic in &diagnostics {
        if diagnostic.is_error() {
            errors += 1;
        } else {
            warnings += 1;
        }

        let file = diagnostic.range.file;
        if current.as_ref().is_none_or(|(f, ..)| *f != file) {
            current = db
                .path(file)
                .and_then(|path| db.vfs().read(file).map(|text| (file, path, text)));
        }

        match &current {
            Some((_, path, text)) => {
                let source =
                    Source { name: path.as_str(), text: &text.text, line_index: &text.line_index };
                // A `while compiling` note may point into another module of
                // the program — the instantiation site — which the database
                // can open.
                let locate = |range: kg_base::FileRange| {
                    let name = db.path(range.file)?.to_string();
                    let text = db.vfs().read(range.file)?;
                    Some(render::Located { name, text })
                };
                let _ = write!(
                    out,
                    "{}",
                    render::render_with(diagnostic, &source, &locate, global.error_format, styles)
                );
            }
            // The two synthetic modules and the loader's own "can't find file"
            // have no text to excerpt (`FileId::SYNTHETIC`). Printing the
            // message alone beats dropping the diagnostic.
            None => {
                let _ = writeln!(out, "{}: {}", styles.error("error"), diagnostic.message);
            }
        }
    }

    if !global.quiet && global.error_format == ErrorFormat::Human {
        let _ = write!(out, "{}", summary(files.len(), errors, warnings, styles));
    }
    // A batch check iterates to the fixpoint, so this is the guard against a
    // loop that does not converge — a bug, and never a silent one.
    if let Some(passes) = db.budget_exhausted(&roots) {
        eprintln!(
            "{}: template instantiation stopped after {passes} program passes without converging; \
             instances it had not decided are missing from this check",
            styles.warning("note"),
        );
    }

    // `-W error` promotes: the reference fails the build on any warning
    // (`Main.hs:98-99`).
    let failed = errors > 0 || unreadable > 0 || (resolved.settings.warnings_as_errors && warnings > 0);
    i32::from(failed)
}

/// `kg fmt`: rewrite sources in the house style, or report which ones would
/// change.
///
/// A file that does not parse is reported and left alone — `docs/spec/style.md`
/// §1.1: the formatter's input is a lossless tree, and there is no tree to lay
/// out when the parser has had to guess. `--check` prints the paths that would
/// change, one per line, so the list pipes into anything.
fn fmt(
    global: &GlobalArgs,
    args: &FmtArgs,
    styles: &Styles,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    // `fmt` takes no source-resolution flags — it never follows an import — but
    // it still wants the manifest, for `[fmt]` and for `project.sources`.
    let Some(resolved) = resolve(global, &SourceArgs::default(), &args.paths, err) else { return 2 };
    let Some(config) = fmt_config(&resolved.settings.fmt, args, err) else { return 2 };

    let files = match collect_inputs(&args.paths, &resolved, err) {
        Ok(files) if files.is_empty() => {
            let _ = writeln!(
                err,
                "kg: no input files. Pass paths, or list them under `project.sources` in kanagawa.toml."
            );
            return 2;
        }
        Ok(files) => files,
        Err(code) => return code,
    };
    if args.stdout && files.len() > 1 {
        let _ = writeln!(
            err,
            "kg: `--stdout` formats one file; {} were given. Without it, `kg fmt` writes in place.",
            files.len()
        );
        return 2;
    }

    let mut changed = 0usize;
    let mut skipped = 0usize;

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            let _ = writeln!(err, "kg: cannot read {path}");
            skipped += 1;
            continue;
        };
        let formatted = match kg_fmt::format_file(&text, &config) {
            Ok(formatted) => formatted,
            Err(error) => {
                let _ = writeln!(err, "{}: {path}: {error}", styles.error("error"));
                skipped += 1;
                continue;
            }
        };

        if args.stdout {
            let _ = write!(out, "{formatted}");
            continue;
        }
        if formatted == text {
            continue;
        }
        changed += 1;
        if args.check {
            let _ = writeln!(out, "{path}");
        } else if let Err(error) = std::fs::write(path, &formatted) {
            let _ = writeln!(err, "kg: cannot write {path}: {error}");
            skipped += 1;
        } else if global.verbose >= 1 {
            let _ = writeln!(out, "formatted {path}");
        }
    }

    if !global.quiet && !args.stdout && global.error_format == ErrorFormat::Human {
        let _ = write!(out, "{}", fmt_summary(args.check, files.len(), changed, skipped, styles));
    }
    // Same convention as `check`: 1 means the command ran and found something.
    // Under `--check` that includes "would change"; without it the rewrite has
    // already happened and only a file it could not handle is a failure.
    i32::from(skipped > 0 || (args.check && changed > 0))
}

/// The manifest's `[fmt]` table with the command line layered on top. `None`
/// means the command line was wrong, which is a usage error.
fn fmt_config(base: &kg_config::Fmt, args: &FmtArgs, err: &mut dyn Write) -> Option<kg_config::Fmt> {
    let mut config = base.clone();
    if let Some(width) = args.line_width {
        if width == 0 {
            let _ = writeln!(err, "kg: --line-width must be at least 1");
            return None;
        }
        config.line_width = width;
    }
    if let Some(indent) = args.indent {
        if indent == 0 || indent > 16 {
            let _ = writeln!(err, "kg: --indent must be between 1 and 16, not {indent}");
            return None;
        }
        config.indent = indent;
    }
    if let Some(newline) = &args.newline {
        config.newline = match newline.to_ascii_lowercase().as_str() {
            "lf" => kg_config::Newline::Lf,
            "crlf" => kg_config::Newline::Crlf,
            other => {
                let _ = writeln!(err, "kg: --newline takes `lf` or `crlf`, not `{other}`");
                return None;
            }
        };
    }
    // The two boolean overrides can only turn a setting on: absent means "as
    // configured", which is what an unset clap flag cannot distinguish from
    // "off".
    config.use_tabs |= args.use_tabs;
    config.sort_imports |= args.sort_imports;
    Some(config)
}

fn fmt_summary(check: bool, files: usize, changed: usize, skipped: usize, styles: &Styles) -> String {
    let file_word = if files == 1 { "file" } else { "files" };
    let mut parts = Vec::new();
    if changed > 0 {
        let verb = if check { "would change" } else { "reformatted" };
        parts.push(styles.warning(&format!("{changed} {verb}")));
    }
    if skipped > 0 {
        parts.push(styles.error(&format!("{skipped} skipped")));
    }
    if parts.is_empty() {
        return format!(
            "{} {files} {file_word}: already formatted\n",
            if check { styles.green("checked") } else { styles.green("formatted") }
        );
    }
    format!("{} {files} {file_word}: {}\n", if check { "checked" } else { "formatted" }, parts.join(", "))
}

fn summary(files: usize, errors: usize, warnings: usize, styles: &Styles) -> String {
    let file_word = if files == 1 { "file" } else { "files" };
    if errors == 0 && warnings == 0 {
        return format!(
            "{} {files} {file_word}: no problems found (the backend's checks run under `kg build`)\n",
            styles.green("checked")
        );
    }
    let mut parts = Vec::new();
    if errors > 0 {
        parts.push(styles.error(&format!("{errors} error{}", plural(errors))));
    }
    if warnings > 0 {
        parts.push(styles.warning(&format!("{warnings} warning{}", plural(warnings))));
    }
    format!("checked {files} {file_word}: {}\n", parts.join(", "))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `-vv`: where every effective setting came from.
fn describe_settings(resolved: &Resolved) -> String {
    let settings = &resolved.settings;
    let mut out = String::new();
    out.push_str(&format!(
        "manifest      {}\n",
        resolved.manifest_path.as_ref().map_or("(none)".to_owned(), Utf8PathBuf::to_string)
    ));
    out.push_str(&format!("profile       {}\n", resolved.profile.as_deref().unwrap_or("(none)")));
    out.push_str(&format!(
        "target-device {}\n",
        settings.target_device.as_str()
    ));
    out.push_str(&format!("implicit-base {}\n", settings.implicit_base));
    out.push_str(&format!(
        "base-library  {}\n",
        settings.base_library.as_ref().map_or("(unset)".to_owned(), Utf8PathBuf::to_string)
    ));
    out.push_str("import search path (in order)\n");
    for dir in settings.import_search_path() {
        out.push_str(&format!("  {dir}\n"));
    }
    out.push_str("warnings\n");
    for kind in kg_diag::WarningKind::ALL {
        let state = if settings.warnings.is_enabled(kind) { "warn" } else { "allow" };
        out.push_str(&format!("  {:<22} {state}\n", kind.name()));
    }
    out.push_str(&format!("  {:<22} {}\n", "(as errors)", settings.warnings_as_errors));
    out
}

/// Command-line paths, or the manifest's `project.sources`. Directories are
/// walked for `.k` files.
fn collect_inputs(
    paths: &[String],
    resolved: &Resolved,
    err: &mut dyn Write,
) -> Result<Vec<Utf8PathBuf>, i32> {
    let requested: Vec<Utf8PathBuf> = if paths.is_empty() {
        resolved.sources.clone()
    } else {
        paths.iter().map(Utf8PathBuf::from).collect()
    };

    let mut files = Vec::new();
    for path in requested {
        if path.as_str().contains('*') {
            let _ = writeln!(
                err,
                "kg: glob patterns are not expanded yet ({path}); list the files explicitly"
            );
            return Err(2);
        }
        if path.is_dir() {
            collect_dir(&path, &mut files);
        } else {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_dir(dir: &Utf8Path, files: &mut Vec<Utf8PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else { continue };
        if path.is_dir() {
            collect_dir(&path, files);
        } else if path.extension() == Some("k") {
            files.push(path);
        }
    }
}

fn dump_command(
    global: &GlobalArgs,
    args: &DumpArgs,
    styles: &Styles,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    // `instances` and `ir` are the dumps whose answer is a *program* fact —
    // a template's instances are decided by the files that import it, and a
    // written name sorts by what the imports expose (`Parser.hs:652-695`) —
    // so a real file goes through the same manifest resolution and `Db` as
    // `kg check`, and only stdin falls back to the one-file view.
    if matches!(args.what, dump::What::Instances | dump::What::Ir) && args.file != "-" {
        let paths = [args.file.clone()];
        let Some(resolved) = resolve(global, &args.source, &paths, err) else { return 2 };
        let db = kg_db::Db::new(resolved.settings.clone());
        let path = Utf8PathBuf::from(&args.file);
        if db.vfs().read_path(&path).is_none() {
            let _ = writeln!(err, "kg: cannot read {path}");
            return 1;
        }
        let root = db.file_id(&path);
        if args.what == dump::What::Ir {
            let program = kg_db::sorted::sorted_program(&db, &[root]);
            let _ = write!(
                out,
                "{}",
                dump::ir_sorted(
                    &program,
                    kg_db::ModuleSource::File(root),
                    db.interner(),
                    args.rewrite,
                )
            );
            return 0;
        }
        let _ = write!(out, "{}", dump::instances_rooted(&db, root));
        return 0;
    }

    let text = if args.file == "-" {
        let mut buffer = String::new();
        match std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer) {
            Ok(_) => buffer,
            Err(error) => {
                let _ = writeln!(err, "kg: cannot read stdin: {error}");
                return 1;
            }
        }
    } else {
        match std::fs::read_to_string(&args.file) {
            Ok(text) => text,
            Err(error) => {
                let _ = writeln!(err, "kg: cannot read {}: {error}", args.file);
                return 1;
            }
        }
    };

    // The one-file dumps skip manifest resolution on purpose, so the flag is
    // read straight off the command line: `kg dump cst --parse-docs FILE` is
    // the door through which a user reaches the doc-comment grammar today.
    let options = kg_syntax::ParseOptions { parse_docs: args.source.parse_docs };
    let rendered = match args.what {
        dump::What::Cst => dump::cst(&text, styles, !args.no_trivia, options),
        dump::What::Ast => dump::ast(&text, styles, options),
        dump::What::Tokens => dump::tokens(&text, styles),
        dump::What::Ir => dump::ir(&text),
        dump::What::Instances => dump::instances_standalone(&text),
    };
    let _ = write!(out, "{rendered}");
    0
}

fn explain_command(args: &ExplainArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    if args.list {
        let _ = write!(out, "{}", explain::list());
        return 0;
    }
    let code = args.code.as_deref().unwrap_or_default();
    match explain::parse_query(code) {
        Some(query) => {
            let _ = write!(out, "{}", explain::explain(&query));
            0
        }
        None => {
            let _ = writeln!(
                err,
                "kg: `{code}` is not an error code or warning name. Try `kg explain --list`."
            );
            2
        }
    }
}

/// `kg lsp`: the language server, over stdio.
///
/// **stdout is the protocol stream.** Nothing else may write to it — a stray
/// `println!` is a malformed message, and the client's failure mode is a silent
/// disconnect rather than an error. Hence `err` for the one diagnostic here, and
/// hence no `--quiet`/`--error-format` handling: those shape stdout.
///
/// `--stdio` is accepted and ignored. It is the only transport, but editors pass
/// it out of habit and rejecting it would break configurations for no gain.
fn lsp(args: &LspArgs, err: &mut dyn Write) -> i32 {
    let _ = args.stdio;
    // A server idles between checks, and an idle allocator returns nothing.
    kg_alloc::purge_eagerly();
    match kg_lsp::run_server() {
        Ok(exit) => exit.code(),
        Err(e) => {
            let _ = writeln!(err, "kg lsp: {e}");
            1
        }
    }
}

/// `kg build`: the frontend, then the walk into the backend — `compile`
/// (`ParseTree.hs:31-41`) with `Main.hs:70-101`'s gates before it. A
/// program the frontend rejects never reaches `InitCompiler`; a program
/// the walk declines names the node kind it stopped at; a backend error is
/// the backend's own output and exit code 1, as the reference's.
///
/// The walk goes into a recording first and the backend replays it in a
/// worker process (`kg_ptbridge::isolate`): a declined program never
/// reaches the library, and a backend that throws or faults is exit 3 and
/// a message, not the end of `kg`.
///
/// `--backend trace` writes the call sequence instead of making the calls
/// (`kg-pttrace`'s format, to `-o <file>` or stdout) — the frontend's
/// output as an artefact, and what `cargo xtask ptdiff` grades.
fn build(global: &GlobalArgs, args: &BuildArgs, styles: &Styles, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(mut resolved) = resolve(global, &args.source, &args.paths, err) else { return 2 };
    resolved.settings.backend.optimize = i64::from(args.optimize);
    resolved.settings.backend.backend =
        if args.backend.eq_ignore_ascii_case("sv") { "Sv".to_owned() } else { "None".to_owned() };
    let files = match collect_inputs(&args.paths, &resolved, err) {
        Ok(files) if files.is_empty() => {
            let _ = writeln!(err, "kg: no input files. Pass paths, or list them under `project.sources` in kanagawa.toml.");
            return 2;
        }
        Ok(files) => files,
        Err(code) => return code,
    };
    let db = kg_db::Db::new(resolved.settings.clone());
    let mut roots = Vec::with_capacity(files.len());
    for path in &files {
        if db.vfs().read_path(path).is_none() {
            let _ = writeln!(err, "kg: cannot read {path}");
            return 2;
        }
        roots.push(db.file_id(path));
    }
    let rejected = db.check(&roots).iter().any(kg_diag::Diagnostic::is_error);
    phases(&db, err);
    if rejected {
        // Rendered by `check`, exactly as `kg check` would; the exit code is its.
        let check_args = CheckArgs { paths: args.paths.clone(), source: args.source.clone() };
        return check(global, &check_args, styles, out, err);
    }

    let mut options = kg_backend_sys::options::OptionValues::harness_defaults();
    options.target_device = resolved.settings.target_device.clone();
    options.optimize = i64::from(args.optimize);
    options.debug = args.debug;
    options.place_iterations = 3000;
    options.cmd_args = crate::legacy::cmd_args_string(&std::env::args().collect::<Vec<_>>());
    let file_names: Vec<String> = db.program(&roots).files.iter().map(|&m| kg_ptbridge::program::file_name(&db, m)).collect();
    let codegen = kg_ptbridge::program::CodegenArgs {
        target: resolved.settings.backend.backend.clone(),
        output: kg_ptbridge::program::output_base_name(args.output.as_deref().unwrap_or(""), &file_names),
        resource_usage: String::new(),
        dgml: String::new(),
        dgml_detailed: String::new(),
    };

    if args.backend.eq_ignore_ascii_case("trace") {
        let sink: Box<dyn Write + Send> = match &args.output {
            Some(path) => match std::fs::File::create(path) {
                Ok(f) => Box::new(f),
                Err(e) => {
                    let _ = writeln!(err, "kg: cannot create {path}: {e}");
                    return 2;
                }
            },
            None => Box::new(std::io::stdout()),
        };
        let mut trace = kg_ptbridge::trace::Trace::new(sink);
        let outcome = kg_ptbridge::program::run(&db, &roots, options, Some(&codegen), &mut trace);
        trace.flush();
        return match outcome.declined {
            Some(d) => declined(&db, &d, styles, err),
            None => 0,
        };
    }
    let Some(library) = kg_backend_sys::locate::backend_library() else {
        let _ = writeln!(err, "{}: {}", styles.error("error"), kg_backend_sys::LoadError::NotFound);
        return 2;
    };
    // The backend opens `<output>…sv` and friends with no directory of its
    // own making, and a missing one surfaces as an iostream failure from
    // C++. `kanagawa` keeps that, as the reference does; `kg build` makes the
    // directory, as a build tool is expected to.
    // `-o out/` names the directory itself; `-o out/top` a prefix in it.
    let output = std::path::Path::new(&codegen.output);
    let dir = if codegen.output.ends_with('/') || codegen.output.ends_with(std::path::MAIN_SEPARATOR) {
        Some(output)
    } else {
        output.parent()
    };
    if let Some(dir) = dir
        && !dir.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        let _ = writeln!(err, "{}: cannot create {}: {e}", styles.error("error"), dir.display());
        return 2;
    }
    // The backend, in its worker, writes to this process's stdout itself.
    let _ = out.flush();
    let backend = std::time::Instant::now();
    let outcome = kg_ptbridge::isolate::compile(&db, &roots, options, &codegen, &library);
    if std::env::var_os("KG_PHASES").is_some() {
        let _ = writeln!(err, "kg phases: the walk and the backend {:.3}s", backend.elapsed().as_secs_f64());
    }
    if let Some(d) = outcome.declined {
        return declined(&db, &d, styles, err);
    }
    match outcome.verdict {
        Some(kg_ptbridge::isolate::Verdict::Compiled) | None => 0,
        Some(kg_ptbridge::isolate::Verdict::Rejected) => 1,
        Some(kg_ptbridge::isolate::Verdict::InitFailed) => {
            let _ = writeln!(err, "{}: the backend rejected the options (InitCompiler returned false)", styles.error("error"));
            1
        }
        Some(kg_ptbridge::isolate::Verdict::LoadFailed) => 2,
        Some(kg_ptbridge::isolate::Verdict::Crashed { how }) => {
            let _ = writeln!(
                err,
                "{}: the backend crashed ({how}); `kg build` has no verdict on this program\n\
                 The frontend accepted it and the walk completed; the crash is in libkanagawa-backend,\n\
                 or in what the walk told it. `kg build --backend trace` writes the calls it was given.",
                styles.error("error")
            );
            3
        }
    }
}

/// `KG_PHASES=1`: the frontend's time by phase, each exclusive of those
/// inside it (`kg_db::stats`), to stderr — trace only. `kg build` adds the
/// walk and the backend's worker after it.
fn phases(db: &kg_db::Db, err: &mut dyn Write) {
    if std::env::var_os("KG_PHASES").is_some() {
        let _ = writeln!(err, "kg phases: {}", db.stats().phases_line());
    }
}

/// A program the walk cannot lower yet, by the node kind it stopped at.
fn declined(db: &kg_db::Db, d: &kg_ptbridge::walk::Declined, styles: &Styles, err: &mut dyn Write) -> i32 {
    let where_ = db
        .path(d.at.head.file)
        .map(|p| format!("{p}:{}", u32::from(d.at.head.range.start())))
        .unwrap_or_else(|| format!("{:?}", d.at.head));
    let _ = writeln!(
        err,
        "{}: `kg build` cannot yet lower this program: {} at {where_}\n\
         The walk into the backend declines what the frontend has not rewritten the way the\n\
         reference does; `cargo xtask ptdiff` counts these. Nothing reached the backend; with\n\
         `--backend trace`, every call before the decline was written.",
        styles.error("error"),
        d.reason
    );
    2
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn run_args(list: &[&str]) -> (i32, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&argv(list), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_documented_command_exists() {
        let names: Vec<String> =
            Cli::command().get_subcommands().map(|c| c.get_name().to_owned()).collect();
        for expected in ["check", "fmt", "lsp", "dump", "explain", "build"] {
            assert!(names.contains(&expected.to_owned()), "missing `kg {expected}`");
        }
    }

    #[test]
    fn the_global_flags_are_global() {
        let cli = Cli::try_parse_from(argv(&[
            "kg", "check", "-C", "kanagawa.toml", "--profile", "test", "-vv", "--color", "never",
            "a.k",
        ]))
        .unwrap();
        assert_eq!(cli.global.manifest_path.as_deref(), Some("kanagawa.toml"));
        assert_eq!(cli.global.profile.as_deref(), Some("test"));
        assert_eq!(cli.global.verbose, 2);
        assert_eq!(cli.global.color, ColorChoice::Never);
    }

    #[test]
    fn help_goes_to_stdout_and_exits_zero() {
        let (code, out, err) = run_args(&["kg", "--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("check"), "{out}");
        assert!(err.is_empty());
    }

    /// The help text must not claim more than `check` delivers. The
    /// wording is allowed to change — this asserts the *admission* survives,
    /// which is the thing that quietly rots when a command grows up. What it
    /// admits changed on 2026-10-10: the frontend's last pass gaps closed
    /// (`conformance --strict` 0 failing), and what a clean `check` still
    /// cannot vouch for is the C++ backend's verdict.
    #[test]
    fn help_admits_what_is_not_implemented() {
        let (_, out, _) = run_args(&["kg", "--help"]);
        assert!(out.contains("not implemented"), "{out}");
        let (_, out, _) = run_args(&["kg", "check", "--help"]);
        assert!(out.contains("backend"), "{out}");
        assert!(out.contains("not a guarantee"), "{out}");
    }

    /// `check` follows imports, so its inputs are roots rather than the unit of
    /// work — a fact the help has to state, because it is why a diagnostic can
    /// name a file the user did not.
    #[test]
    fn check_help_says_inputs_are_program_roots() {
        let (_, out, _) = run_args(&["kg", "check", "--help"]);
        assert!(out.contains("program roots"), "{out}");
    }

    #[test]
    fn a_usage_error_exits_two_on_stderr() {
        let (code, out, err) = run_args(&["kg", "no-such-command"]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert!(!err.is_empty());
    }

    /// `build` runs the frontend before anything reaches the backend: a
    /// file that does not exist is refused the way `check` refuses it, with
    /// nothing on stdout.
    ///
    /// **`lsp` must never be driven from an in-process test.** It now runs the
    /// real server, which takes the process's stdin and blocks until the client
    /// disconnects — the whole suite hangs rather than failing. It is exercised
    /// as a subprocess, with a watchdog, in `tests/cli_process.rs`.
    #[test]
    fn build_refuses_a_missing_file_before_the_backend() {
        let (code, out, err) = run_args(&["kg", "--no-manifest", "build", "a.k"]);
        assert_eq!(code, 2);
        assert!(out.is_empty(), "nothing may go to stdout: {out}");
        assert!(err.contains("a.k"), "{err}");
    }

    #[test]
    fn fmt_rejects_a_newline_it_does_not_know() {
        let (code, out, err) = run_args(&["kg", "--no-manifest", "fmt", "--newline", "cr", "a.k"]);
        assert_eq!(code, 2);
        assert!(out.is_empty(), "{out}");
        assert!(err.contains("`lf` or `crlf`"), "{err}");
    }

    #[test]
    fn fmt_needs_paths_when_there_is_no_manifest_to_supply_them() {
        let (code, _, err) = run_args(&["kg", "--no-manifest", "fmt"]);
        assert_eq!(code, 2);
        assert!(err.contains("no input files"), "{err}");
    }

    #[test]
    fn fmt_config_layers_the_command_line_over_the_manifest() {
        let base = kg_config::Fmt { line_width: 80, ..kg_config::Fmt::default() };
        let cli = Cli::try_parse_from(argv(&[
            "kg", "fmt", "--indent", "2", "--use-tabs", "--newline", "CRLF", "src/",
        ]))
        .unwrap();
        let Command::Fmt(args) = cli.command else { panic!("expected fmt") };
        let mut err = Vec::new();
        let config = fmt_config(&base, &args, &mut err).expect("a valid command line");
        assert_eq!(config.line_width, 80, "not overridden, so the manifest's value stands");
        assert_eq!(config.indent, 2);
        assert!(config.use_tabs);
        assert_eq!(config.newline, kg_config::Newline::Crlf);
        assert!(!config.sort_imports, "an absent flag is not an override");
    }

    #[test]
    fn fmt_accepts_its_whole_flag_surface_today() {
        // The point of accepting these now: the command line does not change
        // when kg-fmt lands.
        let cli = Cli::try_parse_from(argv(&[
            "kg", "fmt", "--check", "--line-width", "120", "--indent", "2", "--use-tabs",
            "--newline", "crlf", "--sort-imports", "src/",
        ]))
        .unwrap();
        let Command::Fmt(args) = cli.command else { panic!("expected fmt") };
        assert!(args.check && args.use_tabs && args.sort_imports);
        assert_eq!(args.line_width, Some(120));
        assert_eq!(args.indent, Some(2));
        assert_eq!(args.newline.as_deref(), Some("crlf"));
    }

    #[test]
    fn build_accepts_the_legacy_shaped_options() {
        let cli = Cli::try_parse_from(argv(&[
            "kg", "build", "--backend", "sv", "-O", "1", "-o", "out/", "--emit", "mlir",
            "-I", "library", "-D", "verbose=true", "-U", "T=uint8", "-W", "all", "top.k",
        ]))
        .unwrap();
        let Command::Build(args) = cli.command else { panic!("expected build") };
        assert_eq!(args.optimize, 1);
        assert_eq!(args.output.as_deref(), Some("out/"));
        assert_eq!(args.emit, ["mlir"]);
        assert_eq!(args.source.import_dir, ["library"]);
        assert_eq!(args.source.warn, ["all"]);
    }

    #[test]
    fn explain_prints_the_frontend_code() {
        let (code, out, _) = run_args(&["kg", "explain", "1"]);
        assert_eq!(code, 0);
        assert!(out.contains("Error 1"), "{out}");
    }

    #[test]
    fn explain_rejects_nonsense_with_a_pointer_to_the_list() {
        let (code, _, err) = run_args(&["kg", "explain", "banana"]);
        assert_eq!(code, 2);
        assert!(err.contains("--list"), "{err}");
    }

    #[test]
    fn dump_reads_stdin_when_asked_for_dash() {
        // Only the argument parse is exercised here; reading stdin under the
        // test harness is covered by the integration test.
        let cli = Cli::try_parse_from(argv(&["kg", "dump", "cst", "-"])).unwrap();
        let Command::Dump(args) = cli.command else { panic!("expected dump") };
        assert_eq!(args.what, dump::What::Cst);
        assert_eq!(args.file, "-");
    }

    #[test]
    fn dump_of_a_missing_file_fails_cleanly() {
        let (code, out, err) = run_args(&["kg", "dump", "cst", "/no/such/file.k"]);
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert!(err.contains("cannot read"), "{err}");
    }
}
