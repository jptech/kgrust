//! `cargo xtask errors` — the reference's own diagnostics, **position and
//! message**, graded against ours over every `expected:1` snippet.
//!
//! `xtask conformance` compares exit codes and the first `Error N` and never a
//! position or a message (`docs/spec/conformance.md` §1.7), because the corpus
//! asserts no more than that. Two compilers can therefore agree on all 2,922
//! assertions while disagreeing about what they say and where — and did: a
//! missing import reported at the wrong `import`, and `nosuch()` reported at
//! the expression here and at the member name there (`docs/journal.md`,
//! 2026-09-06; `oracle/README.md`). `kg-oracle --errors` is the only
//! instrument whose two sides do not share this port's reading of the
//! reference, and this gate is that instrument run over the whole
//! must-reject half of the corpus rather than over one probe file at a time.
//!
//! # What is compared
//!
//! For each snippet the reference must reject, **our first error by source
//! position** must be one the reference reports *anywhere* — same path, line,
//! column and message. Containment rather than first-against-first because
//! the reference reports some errors once per file involved (a circular
//! import lands a bundle in every file of the cycle) and orders its output by
//! tree traversal, neither of which is a disagreement; and our first error
//! rather than any error because the first is what a user reads. A
//! disagreement is graded against the reference's nearest error: same
//! message elsewhere is *position*, same place other words is *message*.
//!
//! A snippet the reference rejects **at parse time** is compared on position
//! alone: this parser recovers and reports `expected …` where megaparsec
//! reports `unexpected … expecting …` (divergence #8), so the messages differ
//! by design and the position is the whole of what can be graded.
//!
//! # What it cannot see, stated
//!
//! * A snippet this port compiles cleanly is *ours silent* — the conformance
//!   harness already books it undecidable. The reference's message is printed
//!   under `--verbose`, which is the attribution the strict-failure surveys
//!   used to do by hand.
//! * Snippets whose ctest flags the oracle driver cannot take are skipped and
//!   counted, never silently dropped. Since 2026-09-23 it takes all the ones
//!   the corpus writes: `--define`, `--using`, `--target-device`, the five
//!   `.options` fields, and `--Werror` with the warning switches — under which
//!   both sides are graded by their first *warning*, as the reference fails
//!   on it (`app/Main.hs:93-96`).
//! * Only the first error per side. A second error that disagrees is invisible
//!   until the first one is fixed.
//!
//! The corpus is read-only, so each snippet is written under `target/` in an
//! otherwise-empty directory, which is also what the reference's own harness
//! does (`syntax-test.py:40-44`) and what keeps the importing file's directory
//! — searched first, `Parser/IO.hs:107` — from resolving anything.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::conformance::{Assertion, Expectation, load_all, settings_for, snippet_path};

/// One error as both sides describe it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Reported {
    /// `<snippet>` for the temp file, otherwise the path relative to the corpus.
    pub(crate) path: String,
    /// 1-based, as the reference prints them.
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) message: String,
}

impl Reported {
    pub(crate) fn position(&self) -> (&str, u32, u32) {
        (&self.path, self.line, self.col)
    }

    pub(crate) fn render(&self) -> String {
        format!("{}:{}:{}: {}", self.path, self.line, self.col, self.message.replace('\n', " ⏎ "))
    }

    /// megaparsec's own grammar error, as opposed to a symbol-table or import
    /// failure the reference routes through the same channel with a message
    /// of its own (`registerSymbolError`, `parseFileFailure`).
    fn is_grammar_error(&self) -> bool {
        self.message.starts_with("unexpected ") || self.message.starts_with("expecting ")
    }
}

/// What the oracle said about one snippet.
#[derive(Clone, Debug)]
pub(crate) enum OracleSide {
    /// The oracle could not be run under these flags.
    Skipped(&'static str),
    /// Rejected by the reference's parser; messages are not comparable.
    Parse(Vec<Reported>),
    /// The frontend's own errors, every one of them.
    Frontend(Vec<Reported>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Verdict {
    Agree,
    ParseAgree,
    ParsePosition,
    /// The reference's *parser* rejects it and this port's does not — a later
    /// pass does. That is divergence #7 by construction: the reference decides
    /// "is `a1` a template?" with a symbol table mid-parse (`Parser.hs:668-680`)
    /// and reports `unexpected '<'`; this parser has none and name resolution
    /// reports the name. Booked to the mechanism, never a disagreement, and
    /// printed under `--verbose` so the class stays visible.
    ParseTimeTable,
    Position,
    Message,
    Both,
    OursSilent,
    OracleSilent,
    Skipped,
}

impl Verdict {
    fn is_disagreement(self) -> bool {
        matches!(self, Verdict::ParsePosition | Verdict::Position | Verdict::Message | Verdict::Both)
    }
}

pub fn run(
    kanagawa_root: &Path,
    filter: Option<&str>,
    max_failures: Option<usize>,
    verbose: bool,
) -> Result<()> {
    let Some(oracle) = crate::ir::find_oracle() else {
        anyhow::bail!("kg-oracle is not built — run oracle/build.sh first");
    };
    let root = kanagawa_root
        .canonicalize()
        .with_context(|| format!("no reference checkout at {}", kanagawa_root.display()))?;

    let mut assertions = load_all(&root)?;
    assertions.retain(|a| a.expectation == Expectation::Code(1));
    if let Some(filter) = filter {
        assertions.retain(|a| a.name.contains(filter));
    }

    // Our side: one `Db`, re-configured when the flags change, exactly as the
    // scoreboard runs it — this gate must grade the same route.
    let ours = our_side(&root, &assertions);

    // The oracle's side, fanned out: each run parses the snippet's whole
    // import closure, and there are ~800 of them.
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("target/xtask-errors");
    let theirs = oracle_side(&oracle, &root, &scratch, &assertions);

    let mut counts = [0usize; 10];
    let mut disagreements: Vec<(String, String, String)> = Vec::new();
    let mut silent: Vec<(String, String)> = Vec::new();
    let mut booked: Vec<(String, String, String)> = Vec::new();
    let mut skipped: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut ledger = crate::ratchet::Ledger::new("errors");
    for set in ["disagree", "silent", "oracle-silent", "skipped"] {
        ledger.set(set);
    }
    for ((a, mine), other) in assertions.iter().zip(&ours).zip(&theirs) {
        let verdict = judge(mine.as_ref(), other);
        counts[verdict as usize] += 1;
        match verdict {
            v if v.is_disagreement() => ledger.add("disagree", &a.name),
            Verdict::OursSilent => ledger.add("silent", &a.name),
            Verdict::OracleSilent => ledger.add("oracle-silent", &a.name),
            Verdict::Skipped => ledger.add("skipped", &a.name),
            _ => {}
        }
        if let OracleSide::Skipped(why) = other {
            *skipped.entry(why).or_default() += 1;
        }
        if verdict.is_disagreement() {
            let their = match (other, mine) {
                (OracleSide::Parse(t) | OracleSide::Frontend(t), Some((m, _))) => {
                    nearest(t, m).map_or_else(String::new, Reported::render)
                }
                _ => String::new(),
            };
            let with_line = |r: Option<&Reported>, text: String| match r {
                Some(r) if r.path == "<snippet>" => {
                    let line = a.source.lines().nth(r.line as usize - 1).unwrap_or("");
                    format!("{text}\n            {}", line.trim_end())
                }
                _ => text,
            };
            let their_at = match (other, mine) {
                (OracleSide::Parse(t) | OracleSide::Frontend(t), Some((m, _))) => nearest(t, m),
                _ => None,
            };
            disagreements.push((
                a.name.clone(),
                with_line(mine.as_ref().map(|(r, _)| r), mine.as_ref().map_or_else(String::new, |(r, _)| r.render())),
                with_line(their_at, their),
            ));
        } else if verdict == Verdict::ParseTimeTable
            && let (Some((m, _)), OracleSide::Parse(t)) = (mine, other)
            && let Some(t) = nearest(t, m)
        {
            booked.push((a.name.clone(), m.render(), t.render()));
        } else if verdict == Verdict::OursSilent
            && let OracleSide::Frontend(t) | OracleSide::Parse(t) = other
            && let Some(r) = t.iter().min_by(|a, b| a.position().cmp(&b.position()))
        {
            silent.push((a.name.clone(), r.render()));
        }
    }

    let total = assertions.len();
    let mut out = String::new();
    let _ = writeln!(out, "{total} must-reject snippets, {} skipped", counts[Verdict::Skipped as usize]);
    for (why, n) in &skipped {
        let _ = writeln!(out, "    {n} skipped: {why}");
    }
    let row = |out: &mut String, label: &str, n: usize, note: &str| {
        let _ = writeln!(out, "  {label:<24}{n:>5}   {note}");
    };
    row(&mut out, "agree", counts[Verdict::Agree as usize], "same position, same message");
    row(&mut out, "parse agree", counts[Verdict::ParseAgree as usize], "the reference rejects at parse time; position compared, messages differ by design");
    row(&mut out, "parse position differs", counts[Verdict::ParsePosition as usize], "");
    row(&mut out, "parse-time table", counts[Verdict::ParseTimeTable as usize], "the reference's parser rejects, a later pass here does — divergence #7, booked");
    row(&mut out, "position differs", counts[Verdict::Position as usize], "same message, different place");
    row(&mut out, "message differs", counts[Verdict::Message as usize], "same place, different words");
    row(&mut out, "both differ", counts[Verdict::Both as usize], "");
    row(&mut out, "ours silent", counts[Verdict::OursSilent as usize], "we compile it cleanly — conformance fails these too");
    row(&mut out, "oracle silent", counts[Verdict::OracleSilent as usize], "the reference frontend reports nothing where we do");
    print!("{out}");
    println!("  {}", crate::oracle_cache::report());

    let shown = if verbose { disagreements.len() } else { disagreements.len().min(10) };
    for (name, mine, their) in disagreements.iter().take(shown) {
        println!("\n  DIFF {name}\n    ours:   {mine}\n    theirs: {their}");
    }
    if disagreements.len() > shown {
        println!("\n  ... and {} more (pass --verbose)", disagreements.len() - shown);
    }
    if verbose {
        for (name, mine, their) in &booked {
            println!("  BOOKED {name}\n    ours:   {mine}\n    theirs: {their}");
        }
        for (name, their) in &silent {
            println!("  SILENT {name}\n    theirs: {their}");
        }
    }

    let failed = disagreements.len();
    // Unfiltered and with a baseline, the ratchet is the verdict: which
    // snippets disagree or fall silent, not how many (`crate::ratchet`).
    if filter.is_none() {
        ledger.total("graded", total);
        for (name, v) in [
            ("agree", Verdict::Agree),
            ("parse-agree", Verdict::ParseAgree),
            ("parse-position", Verdict::ParsePosition),
            ("parse-time-table", Verdict::ParseTimeTable),
            ("position", Verdict::Position),
            ("message", Verdict::Message),
            ("both", Verdict::Both),
            ("ours-silent", Verdict::OursSilent),
            ("oracle-silent", Verdict::OracleSilent),
            ("skipped", Verdict::Skipped),
        ] {
            ledger.total(name, counts[v as usize]);
        }
        if crate::ratchet::enforce(ledger)? && max_failures.is_none() {
            return Ok(());
        }
    }
    match max_failures {
        Some(budget) if failed <= budget => {
            if failed < budget {
                println!("\n{failed} disagreements, under the budget of {budget} — lower it.");
            }
            Ok(())
        }
        Some(budget) => anyhow::bail!("{failed} error disagreements, over the budget of {budget}"),
        None if failed > 0 => anyhow::bail!("{failed} error disagreements with the reference"),
        None => Ok(()),
    }
}

pub(crate) fn judge(mine: Option<&(Reported, bool)>, theirs: &OracleSide) -> Verdict {
    match (mine, theirs) {
        (_, OracleSide::Skipped(_)) => Verdict::Skipped,
        (None, OracleSide::Parse(t) | OracleSide::Frontend(t)) if !t.is_empty() => Verdict::OursSilent,
        // Both silent on a must-reject snippet: the oracle disagrees with the
        // corpus, which is the backend's business, not this gate's.
        (None, _) => Verdict::OracleSilent,
        (Some(_), OracleSide::Parse(t) | OracleSide::Frontend(t)) if t.is_empty() => Verdict::OracleSilent,
        (Some((m, true)), OracleSide::Parse(t)) => {
            if t.iter().any(|r| r.position() == m.position()) { Verdict::ParseAgree } else { Verdict::ParsePosition }
        }
        // The reference's parse-error channel also carries its symbol table's
        // and its import loader's verdicts, each with a message of its own
        // (`Circular dependency`, `Can't find file`, ``x`: undefined symbol`).
        // Those compare like any frontend error; only a grammar error against
        // a later pass of ours is the booked divergence.
        (Some((m, false)), OracleSide::Parse(t)) => {
            if t.iter().any(|r| r == m) {
                Verdict::Agree
            } else if t.iter().all(Reported::is_grammar_error) {
                Verdict::ParseTimeTable
            } else {
                compare(m, t)
            }
        }
        (Some((m, _)), OracleSide::Frontend(t)) => compare(m, t),
    }
}

fn compare(m: &Reported, t: &[Reported]) -> Verdict {
    {
        {
            if t.iter().any(|r| r == m) {
                Verdict::Agree
            } else if t.iter().any(|r| r.message == m.message) {
                Verdict::Position
            } else if t.iter().any(|r| r.position() == m.position()) {
                Verdict::Message
            } else {
                Verdict::Both
            }
        }
    }
}

/// The reference's error a disagreement is shown against: same message if
/// one exists, else same position, else its first by position.
pub(crate) fn nearest<'a>(theirs: &'a [Reported], mine: &Reported) -> Option<&'a Reported> {
    theirs
        .iter()
        .find(|r| r.message == mine.message)
        .or_else(|| theirs.iter().find(|r| r.position() == mine.position()))
        .or_else(|| theirs.iter().min_by(|a, b| a.position().cmp(&b.position())))
}

/// Our first error for a snippet, by position, and whether the parser is the
/// one that reported it; `None` when the snippet compiles clean.
pub(crate) type FirstError = Option<(Reported, bool)>;

/// Our first error for every snippet, spread over threads that each own a
/// `Db` — the same fan-out `oracle_side` gives the reference, and for the
/// same reason: 830 snippets one at a time were 18 of the gate's 20 seconds.
/// A driver only — what a snippet reports is `first_error`, unchanged; the
/// slots come back in assertion order. Consecutive chunks, since assertions
/// arrive grouped by test file and a chunk then switches settings a few
/// times at most.
fn our_side(root: &Path, assertions: &[Assertion]) -> Vec<FirstError> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let chunk = assertions.len().div_ceil(workers * 4).max(1);
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<FirstError>>> = assertions.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(settings_for(root, &[]));
                let mut current: Option<Arc<[String]>> = None;
                loop {
                    let start = next.fetch_add(chunk, Ordering::SeqCst);
                    if start >= assertions.len() {
                        break;
                    }
                    for (i, a) in assertions.iter().enumerate().skip(start).take(chunk) {
                        if current.as_deref() != Some(&*a.options) {
                            db.set_settings(settings_for(root, &a.options));
                            current = Some(Arc::clone(&a.options));
                        }
                        *out[i].lock().expect("no panics hold this lock") = Some(first_error(root, &mut db, a));
                    }
                }
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined").expect("every slot filled")).collect()
}

/// The first error this frontend reports for one snippet, by position, and
/// whether the parser is the one that reported it.
pub(crate) fn first_error(root: &Path, db: &mut kg_db::Db, a: &Assertion) -> FirstError {
    let path = snippet_path(root, a);
    let diagnostics = db.check_snippet(&path, &a.source);
    let parse_errors: std::collections::HashSet<(u32, u32)> = {
        let index = kg_base::LineIndex::new(&a.source);
        kg_syntax::parse_file(&a.source)
            .errors()
            .iter()
            .map(|e| {
                let pos = index.line_col(e.range.start());
                (pos.line + 1, pos.col + 1)
            })
            .collect()
    };
    // `--Werror`: with no error, the reference prints its warnings and fails
    // on them (`app/Main.hs:93-96`), so a warning is what this side reports
    // first; with an error, the warnings are never reached.
    let werror = a.options.iter().any(|o| o == "--Werror") && !diagnostics.iter().any(|d| d.is_error());
    let mut errors: Vec<Reported> = diagnostics
        .iter()
        .filter(|d| d.is_error() || werror)
        .map(|d| {
            let text = db.parse(d.range.file).text().to_owned();
            let index = kg_base::LineIndex::new(&text);
            let pos = index.line_col(d.range.range.start());
            let file = db.path(d.range.file).map_or_else(String::new, |p| p.to_string());
            Reported {
                path: normalize_path(root, Path::new(&file)),
                line: pos.line + 1,
                col: pos.col + 1,
                message: normalize_message(&d.message),
            }
        })
        .collect();
    errors.sort_by(|a, b| a.position().cmp(&b.position()));
    let first = errors.into_iter().next();
    let from_parser = first
        .as_ref()
        .is_some_and(|r| r.path == "<snippet>" && parse_errors.contains(&(r.line, r.col)));
    first.map(|r| (r, from_parser))
}

/// Runs the oracle over every snippet, spread over threads.
fn oracle_side(oracle: &Path, root: &Path, scratch: &Path, assertions: &[Assertion]) -> Vec<OracleSide> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<OracleSide>>> =
        assertions.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= assertions.len() {
                    break;
                }
                let a = &assertions[i];
                let result = match oracle_args(root, &a.options) {
                    Err(why) => OracleSide::Skipped(why),
                    Ok(args) => run_oracle(oracle, root, scratch, i, a, &args),
                };
                *out[i].lock().expect("no panics hold this lock") = Some(result);
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined").expect("every slot filled")).collect()
}

/// The ctest flags translated into the oracle driver's spelling
/// (`oracle/driver/Main.hs`, `parseArgs`), or why they cannot be.
pub(crate) fn oracle_args(root: &Path, options: &[String]) -> std::result::Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    let mut args = options.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (arg, None),
        };
        let mut value = || inline.map(str::to_owned).or_else(|| args.next().map(str::to_owned));
        match flag {
            "--import-dir" => {
                if let Some(v) = value() {
                    let p = Path::new(&v);
                    let p = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
                    out.push("-I".to_owned());
                    out.push(p.to_string_lossy().into_owned());
                }
            }
            "--base-library" => {
                if let Some(v) = value() {
                    out.push("--base".to_owned());
                    out.push(v);
                }
            }
            "--no-implicit-base" => {
                if !inline.is_some_and(|v| v.eq_ignore_ascii_case("false") || v == "0") {
                    out.push("--no-base".to_owned());
                }
            }
            "--parse-docs" => out.push("--parse-docs".to_owned()),
            // `ParseOptions` fields (`app/Main.hs:146-162`), verbatim: a
            // define is `NAME#value` on both command lines.
            "--define" | "--using" | "--target-device" | "--backend" | "--stall" | "--optimize"
            | "--max-threads-default" | "--max-threads-limit" => {
                if let Some(v) = value() {
                    out.push(flag.to_owned());
                    out.push(v);
                }
            }
            // The warning switches only matter under `--Werror`, where the
            // driver prints the warnings they enable.
            "--Werror" | "--Wall" => out.push(flag.to_owned()),
            f if f.starts_with("--W") => out.push(f.to_owned()),
            // Job counts and backend-only switches are the harness's.
            _ => {}
        }
    }
    Ok(out)
}

fn run_oracle(oracle: &Path, root: &Path, scratch: &Path, i: usize, a: &Assertion, args: &[String]) -> OracleSide {
    let stem = a.file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let dir = scratch.join(format!("{stem}-{i}"));
    let file = dir.join("syntax_test_temp_file.k");
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, &a.source).is_err() {
        return OracleSide::Skipped("could not write the scratch snippet");
    }
    let mut all: Vec<String> = args.to_vec();
    all.push("--errors".to_owned());
    all.push(file.to_string_lossy().into_owned());
    // Replayed when nothing the oracle reads has changed (`oracle_cache`).
    let output = crate::oracle_cache::run(oracle, root, &all, Some(&dir), &a.source);
    // `KG_ERRORS_KEEP=1` leaves the scratch snippets behind for a by-hand rerun.
    if std::env::var_os("KG_ERRORS_KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let Ok(output) = output else { return OracleSide::Skipped("the oracle could not be spawned") };
    read_oracle(root, &output.stdout, &output.stderr)
}

/// One `kg-oracle --errors` run's output, read: the reading this gate, the
/// differential fuzzer and the reducer share.
pub(crate) fn read_oracle(root: &Path, stdout: &str, stderr: &str) -> OracleSide {
    if let Some(rest) = stderr.split_once("PARSE-ERROR\n").map(|(_, r)| r) {
        return OracleSide::Parse(parse_bundles(root, rest));
    }
    OracleSide::Frontend(parse_bundles(root, stdout))
}

/// `errorBundlePretty` output, one bundle after another:
///
/// ```text
/// <path>:<line>:<col>:
///   |
/// N | source line
///   | ^
/// message, possibly several lines
/// <blank>
/// ```
fn parse_bundles(root: &Path, text: &str) -> Vec<Reported> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        // A `note: path:l:c:` frame belongs to the bundle above it (the
        // `while compiling` sites, `Error.hs:176-195`), never opens one.
        let Some((path, line, col)) = header(lines[i]).filter(|_| !lines[i].starts_with("note: ")) else {
            i += 1;
            continue;
        };
        i += 1;
        // The gutter: `  |`, `N | …`, `  | ^`.
        while i < lines.len() && is_gutter(lines[i]) {
            i += 1;
        }
        // `ImportError` (`Error.hs:60`): the outer header is the `import`
        // keyword and the real error is a whole bundle nested under
        // `In imported file: <path>:l:c:`, one level per hop. The innermost
        // location and message are the comparable unit — this port reports
        // the error where it is, not where it was imported from.
        let (mut path, mut line, mut col) = (path, line, col);
        while i < lines.len()
            && let Some(rest) = lines[i].strip_prefix("In imported file: ")
            && let Some((p, l, c)) = header(rest)
        {
            (path, line, col) = (p, l, c);
            i += 1;
            while i < lines.len() && is_gutter(lines[i]) {
                i += 1;
            }
        }
        let mut message = String::new();
        while i < lines.len()
            && !lines[i].trim().is_empty()
            && header(lines[i]).is_none()
            && !lines[i].starts_with("note: ")
            && lines[i] != WERROR_TAIL
        {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(lines[i].trim_end());
            i += 1;
        }
        let message = bare_warning(message);
        out.push(Reported {
            path: normalize_path(root, Path::new(path)),
            line,
            col,
            message: normalize_message(&message),
        });
    }
    out
}

/// The line `--Werror` ends the reference's output with, after the warnings
/// (`app/Main.hs:96`) — not part of any bundle.
pub(crate) const WERROR_TAIL: &str = "Error 1: Warnings treated as errors";

/// A warning printed under `--Werror` (`app/Main.hs:93-96`) wears the
/// severity and its switch — `warning: … [--Wno-shadow]`, the
/// `errorBundlePretty` of a `WarningF` — where this port's message is the
/// bare text.
pub(crate) fn bare_warning(message: String) -> String {
    let Some(rest) = message.strip_prefix("warning: ") else { return message };
    let (first, tail) = rest.split_once('\n').map_or((rest, None), |(f, t)| (f, Some(t)));
    let first = match first.rsplit_once(" [--W") {
        Some((text, switch)) if switch.ends_with(']') => text,
        _ => first,
    };
    tail.map_or_else(|| first.to_owned(), |t| format!("{first}\n{t}"))
}

pub(crate) fn header(line: &str) -> Option<(&str, u32, u32)> {
    let rest = line.strip_suffix(':')?;
    let (rest, col) = rest.rsplit_once(':')?;
    let (path, ln) = rest.rsplit_once(':')?;
    if path.is_empty() || path.starts_with(' ') {
        return None;
    }
    Some((path, ln.parse().ok()?, col.parse().ok()?))
}

pub(crate) fn is_gutter(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('|') || t.split_once(' ').is_some_and(|(n, rest)| n.parse::<u32>().is_ok() && rest.starts_with('|'))
}

/// The snippet's temp file is `<snippet>` on both sides, whatever directory
/// each harness put it in; everything else is relative to the corpus root.
pub(crate) fn normalize_path(root: &Path, path: &Path) -> String {
    if path.file_name().is_some_and(|n| n == "syntax_test_temp_file.k") {
        return "<snippet>".to_owned();
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canon.strip_prefix(root).unwrap_or(&canon).to_string_lossy().into_owned()
}

/// Trailing whitespace per line and at the end; the reference's `unlines`
/// terminates a directory list with a newline, and this port's does not.
///
/// A "Perhaps you meant one of these:" list is compared as a **set**: the
/// reference's order is `HashMap.toList` (`Frontend.hs:136-141`) — its own
/// hash function's iteration order, which nothing outside GHC can reproduce
/// and which the reference itself does not promise. This port sorts the list
/// (`undefined_symbols::hints`); *which* names are offered is still graded.
pub(crate) fn normalize_message(m: &str) -> String {
    // A message can embed the snippet's path (`See previous declaration at …`);
    // each harness put the temp file somewhere else.
    let snippet = |w: &str| {
        if w.contains("syntax_test_temp_file.k") {
            let tail = w.rsplit_once("syntax_test_temp_file.k").map_or("", |(_, t)| t);
            format!("<snippet>{tail}")
        } else {
            w.to_owned()
        }
    };
    let owned: Vec<String> = m
        .lines()
        .map(|l| l.split(' ').map(snippet).collect::<Vec<_>>().join(" ").trim_end().to_owned())
        .collect();
    let mut lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    if let Some(i) = lines.iter().position(|l| l.ends_with("Perhaps you meant one of these:")) {
        let hints = &mut lines[i + 1..];
        let tail = hints.iter().position(|l| !l.starts_with('\t')).unwrap_or(hints.len());
        hints[..tail].sort_unstable();
    }
    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_parses_to_its_header_and_message() {
        let text = "/x/syntax_test_temp_file.k:2:1:\n  |\n2 | import missing\n  | ^\nCan't find file: missing.k in any of the import directories\n/lib\n\n";
        let r = parse_bundles(Path::new("/nowhere"), text);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].path, "<snippet>");
        assert_eq!((r[0].line, r[0].col), (2, 1));
        assert_eq!(r[0].message, "Can't find file: missing.k in any of the import directories\n/lib");
    }

    #[test]
    fn two_bundles_back_to_back_are_two_errors() {
        let text = "/x/syntax_test_temp_file.k:14:16:\n   |\n14 |     auto z = p.c;\n   |                ^\nUndefined member `c` of type `P`\n\n/x/syntax_test_temp_file.k:15:5:\n   |\n15 |     undefined_fn(1);\n   |     ^\nSymbol name not in scope: `undefined_fn`\n\n";
        let r = parse_bundles(Path::new("/nowhere"), text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].message, "Symbol name not in scope: `undefined_fn`");
    }

    #[test]
    fn a_source_line_that_looks_like_a_header_is_not_one() {
        // A gutter line is skipped before the message scan starts, so a source
        // line ending in `:` cannot be mistaken for a bundle header.
        let text = "/x/a.k:1:1:\n  |\n1 | case 0:\n  | ^\nmsg\n";
        let r = parse_bundles(Path::new("/"), text);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].message, "msg");
    }

    #[test]
    fn hint_lists_compare_as_sets() {
        let a = normalize_message("Symbol name not in scope: `x`\nPerhaps you meant one of these:\n\t`b`\n\t`a`");
        let b = normalize_message("Symbol name not in scope: `x`\nPerhaps you meant one of these:\n\t`a`\n\t`b`");
        assert_eq!(a, b);
        // A single hint is not a list and is left alone.
        assert_eq!(normalize_message("m\nPerhaps you meant: `a`"), "m\nPerhaps you meant: `a`");
    }

    /// Under `--Werror` the oracle prints warning bundles: the severity and
    /// the switch are framing, a `while compiling` note is a note, and the
    /// closing `Error 1:` line belongs to no bundle.
    #[test]
    fn a_werror_warning_parses_to_its_bare_message() {
        let text = "/x/syntax_test_temp_file.k:5:16:\n  |\n5 |   uint32 x = 2;\n  |          ^\nwarning: This declaration of `x` shadows the existing declaration [--Wno-shadow]\nnote: /x/syntax_test_temp_file.k:3:5:\n  |\n3 |   uint32 x = 1;\n  |   ^\nwhile compiling\n\nError 1: Warnings treated as errors\n";
        let r = parse_bundles(Path::new("/r"), text);
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!((r[0].line, r[0].col), (5, 16));
        assert_eq!(r[0].message, "This declaration of `x` shadows the existing declaration");
    }

    #[test]
    fn flags_translate_and_the_untranslatable_skip() {
        let root = Path::new("/r");
        let ok = oracle_args(root, &["--import-dir=/lib".into(), "--base-library".into(), "b.k".into(), "--Wno-shadow".into()]).unwrap();
        assert_eq!(ok, vec!["-I", "/lib", "--base", "b.k", "--Wno-shadow"]);
        assert_eq!(oracle_args(root, &["--define=X#1".into()]).unwrap(), vec!["--define", "X#1"]);
        assert_eq!(oracle_args(root, &["--target-device".into(), "mock".into()]).unwrap(), vec!["--target-device", "mock"]);
        assert_eq!(oracle_args(root, &["--Wall".into(), "--Werror".into()]).unwrap(), vec!["--Wall", "--Werror"]);
        assert_eq!(oracle_args(root, &["--stall=3".into()]).unwrap(), vec!["--stall", "3"]);
        assert_eq!(oracle_args(root, &["--no-implicit-base=false".into()]).unwrap(), Vec::<String>::new());
    }
}
