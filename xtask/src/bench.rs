//! `cargo xtask bench` — a staged benchmark over the reference corpus.
//!
//! Two questions this answers that a single throughput number cannot:
//!
//! * **Which stage got slower?** Lexing, tree building, formatting and
//!   checking are timed separately, so a regression lands on a stage rather
//!   than on "the compiler".
//! * **Will the editor feel it?** Every stage reports its worst *single file*
//!   as well as its aggregate rate. Aggregate throughput is a batch-compile
//!   number; an editor's budget is spent on one file, and the largest file in
//!   the corpus (`library/numeric/float32.k`, 3,904 lines) is the ceiling a
//!   user actually feels. A frame is ~16 ms.
//!
//! Timing is wall clock over repeated passes with a warm-up, reporting the
//! *median* pass rather than the mean, because a single scheduler hiccup
//! should not look like a regression. This is deliberately not `criterion`:
//! the interesting axis here is a fixed 2.7 MiB corpus rather than one
//! microbenchmark, and a dependency that pulls in its own harness earns less
//! than it costs.

use std::{
    hint::black_box,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;

/// One measured stage.
struct Stage {
    name: &'static str,
    /// What the stage is fed, for the report's `MiB/s` denominator.
    bytes: usize,
    /// Median wall time for one pass over every file.
    total: Duration,
    /// The slowest single file by its own *median* time across passes, and
    /// which. Not the maximum: a max-over-passes reports the unluckiest sample
    /// of N, which grows with N and drowns a real change in scheduler noise.
    /// This cost an afternoon once — a 7% improvement was plainly visible in a
    /// warm-cache profile and invisible here, because "worst" was measuring
    /// variance rather than the file.
    worst: (Duration, String),
    /// Files this stage could actually run on. Formatting and checking skip
    /// files that do not parse, and averaging over a denominator that includes
    /// them would flatter both.
    files: usize,
}

impl Stage {
    fn mib_per_s(&self) -> f64 {
        self.bytes as f64 / 1_048_576.0 / self.total.as_secs_f64()
    }
}

const PASSES: usize = 5;

pub fn run(kanagawa_root: &Path, filter: Option<&str>, json: bool, keystrokes: usize, recorded: bool) -> Result<()> {
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    let corpus: Vec<(String, String)> = files
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).ok()?;
            Some((rel(p, kanagawa_root), text))
        })
        .collect();
    if corpus.is_empty() {
        anyhow::bail!("no readable .k files under {}", kanagawa_root.display());
    }

    let wanted = |name: &str| filter.is_none_or(|f| name.contains(f));
    let mut stages = Vec::new();

    if wanted("lex") {
        stages.push(measure("lex", &corpus, |text| {
            black_box(kg_parser::LexedStr::new(text));
        }));
    }
    if wanted("parse") {
        // Lex + grammar + tree build: what `parse_file` costs a caller.
        stages.push(measure("parse", &corpus, |text| {
            black_box(kg_syntax::parse_file(text));
        }));
    }
    if wanted("format") {
        let config = kg_config::Fmt::default();
        stages.push(measure_some("format", &corpus, |text| {
            kg_fmt::format_file(text, &config).ok().map(black_box).is_some()
        }));
    }
    if wanted("check") {
        // The whole frontend over each file as a program alone — no imports
        // and no base library, the program-wide loop included
        // (`kg_db::check_source`, a `Db` since sprint W3.7; before that a
        // one-file door with passes of its own, so readings across that
        // change are not comparable). Not what any user runs: `keystroke`
        // below is.
        stages.push(measure("check", &corpus, |text| {
            black_box(kg_db::check_source(text));
        }));
    }
    if wanted("keystroke") || wanted("fixpoint") {
        // **What the editor actually costs.** A rooted `Db` with the base
        // library loaded, one file re-entered as though it had just been typed
        // in, and the whole program re-checked.
        //
        // Two rows from one run (sprint W7.6): `keystroke`, the time to the
        // editor's first answer — the loop at four rounds, what the gutter
        // shows first and what this row measured before the editor ran on
        // to the fixpoint — and `fixpoint`, the time to its last, `kg
        // check`'s, published after. Where the loop ends by four rounds the
        // two are one answer and one time.
        //
        // This is the number that matters now that `kg-lsp` publishes frontend
        // diagnostics: every stage above it measures a component, and none of
        // them measures the loop a person is waiting on. It is deliberately
        // *not* a cold start — the base library and every unedited import stay
        // memoized, because they do in the server too, and a benchmark that
        // reloads them measures a case that happens once per session.
        let mut db = kg_db::Db::new(rooted_settings(kanagawa_root));
        // The server's budget, not a batch compile's: this stage is the
        // language server's loop.
        db.set_pass_budget(kg_db::PassBudget::EDITOR);
        let root = camino::Utf8PathBuf::from_path_buf(kanagawa_root.to_path_buf())
            .expect("the reference path is UTF-8");
        let mut paths = corpus.iter().map(|(name, _)| root.join(name)).cycle();
        let mut nth = 0usize;
        let (first, last) = measure_layered("keystroke", "fixpoint", &corpus, move |text| {
            let path = paths.next().expect("cycle never ends");
            nth += 1;
            // **The text has to change.** `set_file_text` bumps the revision, but
            // a revision bump is not an edit: `Db::parse_module` keeps the old
            // `Arc<Parse>` when the reparse compares equal, `Db::item_tree` is
            // keyed on that `Arc`'s identity and `Db::type_table` on the item
            // tree's, so feeding a file its own bytes back measures a `didChange`
            // that invalidated nothing below the parser.
            //
            // That was harmless while `passes::run` rebuilt the type table on
            // every check — the stage paid for inference either way. It stopped
            // being harmless the moment inference got a memo, and this stage
            // reported a 25% win that was the fixpoint leaving the *measurement*.
            // `crates/kg-db/tests/query.rs`'s
            // `re_entering_identical_text_leaves_the_type_table_memoized` pins the
            // fact this comment depends on.
            //
            // One or two trailing spaces, alternating, so consecutive passes over
            // the same file always differ — the parse memo holds one entry per
            // module, so a constant suffix would go stale exactly once and then
            // hit forever. Trailing whitespace changes no declaration, so the
            // work measured is the work a real edit costs.
            let t0 = Instant::now();
            let file = db.set_file_text(&path, format!("{text}{}", " ".repeat(nth % 2 + 1)));
            let mut first = None;
            black_box(db.diagnostics_layered(&[file], file, |d| {
                black_box(d);
                first = Some(t0.elapsed());
            }));
            let last = t0.elapsed();
            (first.unwrap_or(last), last)
        });
        if wanted("keystroke") {
            stages.push(first);
        }
        stages.push(last);
    }

    let phases = wanted("phases");
    // Its own filter as well, since a `--filter keystroke` matching it would
    // add its lines to what `gates.sh` greps the stage's row from.
    let keystroke_table = phases || filter == Some("keystroke-phases");
    if keystroke_table {
        keystroke_phases(&kanagawa_root.canonicalize()?, keystrokes, recorded)?;
    }
    if phases {
        phases_table(kanagawa_root)?;
    }
    let cancel = wanted("cancel");
    if cancel {
        cancel_stage(kanagawa_root)?;
    }
    if stages.is_empty() {
        if keystroke_table || cancel {
            return Ok(());
        }
        anyhow::bail!("no stage matched --filter");
    }

    if json {
        print_json(&corpus, &stages);
    } else {
        print_table(&corpus, &stages);
    }
    Ok(())
}

/// The files W0.5's keystroke table prices: the bench's worst.
const PHASE_FILES: &[&str] = &["test/library/processor/risc-v.k", "test/library/data/array.k"];

/// **What a keystroke spends, by phase and by edit** (sprint W0.5): for
/// each of [`PHASE_FILES`], a warm `Db` under the recorded compile's
/// settings — not [`rooted_settings`], under which `risc-v.k` is a program
/// the reference rejects — and each of `crate::edits`' four edits and its
/// undo, under the editor's budget and a batch compile's. Per step: the
/// wall time of the edited file's diagnostics — under the editor's, to its
/// first answer at four rounds and to its last at the fixpoint
/// (`Db::diagnostics_layered`, sprint W7.6) — the exclusive time of every
/// phase that took any (`kg_db::stats`; the editor's loop times its own on
/// its thread), and every query that missed. One pass, not a median: the
/// rows are for reading where the time goes, and the machine is bimodal
/// under load (`docs/gates.md`, `bench`).
fn phases_table(kanagawa_root: &Path) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let programs = crate::ptrace::programs(kanagawa_root)?;
    let library = kanagawa_root.join("library");
    for file in PHASE_FILES {
        let Some(p) = programs.iter().find(|p| p.name == format!("file::{file}")) else { continue };
        let settings = crate::conformance::settings_for(kanagawa_root, &p.options);
        let path = camino::Utf8PathBuf::from_path_buf(kanagawa_root.join(file)).expect("UTF-8 paths");
        let text = std::fs::read_to_string(&path)?;
        for (budget, label) in [
            (kg_db::PassBudget::EDITOR, "the editor: its first answer, then its last"),
            (kg_db::PassBudget::Fixpoint, "a batch compile's fixpoint"),
        ] {
            let mut db = kg_db::Db::new(settings.clone());
            db.set_pass_budget(budget);
            let t0 = Instant::now();
            let root = db.set_file_text(&path, text.clone());
            let (first, last) = layered(&db, root, t0);
            println!("\n{file}, {label}: cold {}", answered(first, last));
            let imported = crate::edits::library_target(&db, root, &library);
            for kind in crate::edits::Kind::ALL {
                let (target, before, after) = match kind {
                    crate::edits::Kind::LibraryBody => {
                        let Some(t) = imported.clone() else { continue };
                        t
                    }
                    _ => {
                        let Some(after) = crate::edits::apply(kind, &text) else { continue };
                        (path.clone(), text.clone(), after)
                    }
                };
                for (step, content) in [(kind.name().to_owned(), after), (format!("{} undone", kind.name()), before)] {
                    let s0 = db.stats();
                    let t0 = Instant::now();
                    db.set_file_text(&target, content);
                    let (first, last) = layered(&db, root, t0);
                    let d = db.stats().since(&s0);
                    println!("  {step:<22} {}  {}", answered(first, last), d.phases_line());
                    println!("  {:<22} {:>7}  missed: {}", "", "", d.misses_line());
                }
            }
        }
    }
    Ok(())
}

/// `root`'s diagnostics as the editor publishes them: the seconds from
/// `t0` to the first answer, where there is one before the last, and to
/// the last.
fn layered(db: &kg_db::Db, root: kg_base::FileId, t0: Instant) -> (Option<f64>, f64) {
    let mut first = None;
    black_box(db.diagnostics_layered(&[root], root, |d| {
        black_box(d);
        first = Some(t0.elapsed().as_secs_f64());
    }));
    (first, t0.elapsed().as_secs_f64())
}

/// `first 0.88s, last 2.31s`, or the last alone where it was the only
/// answer.
fn answered(first: Option<f64>, last: f64) -> String {
    match first {
        Some(first) => format!("first {first:>5.2}s, last {last:>5.2}s"),
        None => format!("{:>13}  {last:>5.2}s", "only"),
    }
}

/// **The `keystroke` stage's own number, by phase**: each of
/// [`PHASE_FILES`] as that stage measures it — [`rooted_settings`], the
/// editor's budget, a trailing space alternated — `keystrokes` times after
/// a warm-up: each keystroke's wall time to the editor's first answer and
/// to its last (sprint W7.6), and the phases of all of them together. The table below it runs under the recorded compile's
/// settings, which are another program for `risc-v.k`; this is the one the
/// stage's worst file reports. Under load the phases' proportions read
/// better than their sum, and `/usr/bin/time -l` over `--keystrokes 10`
/// against `--keystrokes 0` counts the instructions ten keystrokes take.
///
/// `recorded` runs the same keystrokes under each file's recorded compile
/// settings instead — the full base library, what a project's `kg lsp`
/// serves — where `risc-v.k`'s loop is three times the stage's.
fn keystroke_phases(kanagawa_root: &Path, keystrokes: usize, recorded: bool) -> Result<()> {
    let programs = if recorded { crate::ptrace::programs(kanagawa_root)? } else { Vec::new() };
    for file in PHASE_FILES {
        let path = camino::Utf8PathBuf::from_path_buf(kanagawa_root.join(file)).expect("UTF-8 paths");
        let text = std::fs::read_to_string(&path)?;
        let settings = if recorded {
            let Some(p) = programs.iter().find(|p| p.name == format!("file::{file}")) else { continue };
            crate::conformance::settings_for(kanagawa_root, &p.options)
        } else {
            rooted_settings(kanagawa_root)
        };
        let mut db = kg_db::Db::new(settings);
        db.set_pass_budget(kg_db::PassBudget::EDITOR);
        let root = db.set_file_text(&path, text.clone());
        black_box(db.diagnostics(&[root], root));
        let s0 = db.stats();
        let mut firsts = Vec::with_capacity(keystrokes);
        let mut lasts = Vec::with_capacity(keystrokes);
        for nth in 0..keystrokes {
            let t0 = Instant::now();
            let root = db.set_file_text(&path, format!("{text}{}", " ".repeat(nth % 2 + 1)));
            let (first, last) = layered(&db, root, t0);
            firsts.push(first.unwrap_or(last));
            lasts.push(last);
        }
        if lasts.is_empty() {
            continue;
        }
        let d = db.stats().since(&s0);
        let label = if recorded { "a keystroke under the recorded settings" } else { "the keystroke stage" };
        let ms = |walls: &[f64]| -> (f64, String) {
            let row: Vec<String> = walls.iter().map(|w| format!("{:.0}", w * 1000.0)).collect();
            let mut sorted = walls.to_vec();
            sorted.sort_by(f64::total_cmp);
            (sorted[sorted.len() / 2] * 1000.0, row.join(", "))
        };
        let ((first, first_row), (last, last_row)) = (ms(&firsts), ms(&lasts));
        println!("\n{file}, {label}: the first answer's median {first:.0} ms of [{first_row}] ms");
        println!("  the last answer's median {last:.0} ms of [{last_row}] ms");
        println!("  all {keystrokes}: {}", d.phases_line());
        println!("  missed: {}", d.misses_line());
        // What the editor keeps of its last run for the next (sprint
        // W7.5d, `kg_db::snapshot`), beyond the program it shares.
        if let Some(s) = db.snapshot_size(&[root]) {
            println!(
                "  the kept snapshot: a program of {} nodes; logs of {} nodes and {} types, keys holding {} positions — {:.1} MB beside it",
                s.nodes,
                s.logged_nodes,
                s.logged_types,
                s.key_positions,
                s.bytes as f64 / 1e6
            );
        }
    }
    Ok(())
}

/// **What a superseded check costs** (sprint W7.1, W7.2): `risc-v.k`'s
/// check, started on one thread and its `Db`'s token raised from another at
/// several points into it; the time from the raise to the check's return.
/// Twice: a batch compile's check at the fixpoint, and the editor's
/// (`Db::diagnostics_layered`, sprint W7.6), whose loop runs on a thread of
/// its own and whose reader waits for it — raised before its first answer,
/// and after it, while the fixpoint runs on. The exit is 50 ms.
fn cancel_stage(kanagawa_root: &Path) -> Result<()> {
    for budget in [kg_db::PassBudget::Fixpoint, kg_db::PassBudget::EDITOR] {
        cancel_under(kanagawa_root, budget)?;
    }
    Ok(())
}

fn cancel_under(kanagawa_root: &Path, budget: kg_db::PassBudget) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let file = PHASE_FILES[0];
    let programs = crate::ptrace::programs(kanagawa_root)?;
    let Some(p) = programs.iter().find(|p| p.name == format!("file::{file}")) else { anyhow::bail!("no program {file}") };
    let mut db = kg_db::Db::new(crate::conformance::settings_for(kanagawa_root, &p.options));
    db.set_pass_budget(budget);
    let path = camino::Utf8PathBuf::from_path_buf(kanagawa_root.join(file)).expect("UTF-8 paths");
    let text = std::fs::read_to_string(&path)?;
    let mut worst = Duration::ZERO;
    let mut row = Vec::new();
    for (n, after_ms) in [100u64, 500, 1500, 3000, 4500].into_iter().enumerate() {
        // New text each time, so the check has work to be stopped in.
        let root = db.set_file_text(&path, format!("{text}{}", " ".repeat(n + 1)));
        let token = db.cancel_token();
        let db = &db;
        let (outcome, latency) = std::thread::scope(|scope| {
            let check = scope.spawn(move || {
                let r = kg_base::cancel::catch(|| black_box(db.diagnostics_layered(&[root], root, |d| drop(black_box(d)))));
                (r.is_err(), Instant::now())
            });
            std::thread::sleep(Duration::from_millis(after_ms));
            let raised = Instant::now();
            token.raise();
            let (cancelled, returned) = check.join().expect("the check does not panic");
            (cancelled, returned.saturating_duration_since(raised))
        });
        if outcome {
            worst = worst.max(latency);
        }
        row.push(format!("{after_ms} ms in: {}", if outcome { format!("{:.1} ms", latency.as_secs_f64() * 1000.0) } else { "finished first".to_owned() }));
    }
    let label = if budget.first_answer().is_some() { "the editor's" } else { "a batch compile's" };
    println!("\ncancel ({file}, {label}): {}", row.join(", "));
    println!("worst {:.1} ms from the raise to the check's return — {} the 50 ms exit", worst.as_secs_f64() * 1000.0, if worst.as_millis() <= 50 { "within" } else { "OVER" });
    Ok(())
}

/// [`measure`] for a stage whose one run answers twice: `f` returns the
/// time to each answer, and each makes a stage of its own.
fn measure_layered(
    first: &'static str,
    last: &'static str,
    corpus: &[(String, String)],
    mut f: impl FnMut(&str) -> (Duration, Duration),
) -> (Stage, Stage) {
    let bytes: usize = corpus.iter().map(|(_, t)| t.len()).sum();
    for (_, text) in corpus {
        f(text);
    }
    let mut totals = [Vec::with_capacity(PASSES), Vec::with_capacity(PASSES)];
    let mut per_file: [Vec<Vec<Duration>>; 2] = std::array::from_fn(|_| vec![Vec::with_capacity(PASSES); corpus.len()]);
    for _ in 0..PASSES {
        let mut total = [Duration::ZERO; 2];
        for (i, (_, text)) in corpus.iter().enumerate() {
            let (a, b) = f(text);
            for (k, d) in [a, b].into_iter().enumerate() {
                total[k] += d;
                per_file[k][i].push(d);
            }
        }
        for k in 0..2 {
            totals[k].push(total[k]);
        }
    }
    let stage = |k: usize, name: &'static str| {
        let mut sorted = totals[k].clone();
        sorted.sort();
        let mut worst = (Duration::ZERO, String::new());
        for (i, samples) in per_file[k].iter().enumerate() {
            let mut samples = samples.clone();
            samples.sort();
            let median = samples[samples.len() / 2];
            if median > worst.0 {
                worst = (median, corpus[i].0.clone());
            }
        }
        Stage { name, bytes, total: sorted[PASSES / 2], worst, files: corpus.len() }
    };
    (stage(0, first), stage(1, last))
}

/// Runs `f` over every file, `PASSES` times, and keeps the median pass.
fn measure(name: &'static str, corpus: &[(String, String)], mut f: impl FnMut(&str)) -> Stage {
    measure_some(name, corpus, |t| {
        f(t);
        true
    })
}

/// As [`measure`], but `f` returns whether it did any work — a stage that
/// declines a file should not have that file counted against its throughput.
fn measure_some(
    name: &'static str,
    corpus: &[(String, String)],
    mut f: impl FnMut(&str) -> bool,
) -> Stage {
    // Warm up: first-touch page faults and branch predictors are not the thing
    // under test.
    let mut ran = 0usize;
    let mut bytes = 0usize;
    for (_, text) in corpus {
        if f(text) {
            ran += 1;
            bytes += text.len();
        }
    }

    let mut totals = Vec::with_capacity(PASSES);
    // Per-file samples, so each file's own median can be taken.
    let mut per_file: Vec<Vec<Duration>> = vec![Vec::with_capacity(PASSES); corpus.len()];
    for _ in 0..PASSES {
        let mut total = Duration::ZERO;
        for (i, (_, text)) in corpus.iter().enumerate() {
            let t0 = Instant::now();
            let did = f(text);
            let dt = t0.elapsed();
            if !did {
                continue;
            }
            total += dt;
            per_file[i].push(dt);
        }
        totals.push(total);
    }
    totals.sort();

    let mut worst = (Duration::ZERO, String::new());
    for (i, samples) in per_file.iter_mut().enumerate() {
        if samples.is_empty() {
            continue;
        }
        samples.sort();
        let median = samples[samples.len() / 2];
        if median > worst.0 {
            worst = (median, corpus[i].0.clone());
        }
    }

    Stage { name, bytes, total: totals[PASSES / 2], worst, files: ran }
}

/// The reference's own layout, as `test/syntax/CMakeLists.txt:40-44` and
/// `syntax-test.py:106` assemble it. Without it the base library does not
/// resolve and the `keystroke` stage measures a program of one file.
pub(crate) fn rooted_settings(kanagawa_root: &Path) -> kg_config::Settings {
    let root = camino::Utf8PathBuf::from_path_buf(
        kanagawa_root.canonicalize().unwrap_or_else(|_| kanagawa_root.to_path_buf()),
    )
    .expect("the reference path is UTF-8");
    let mut settings = kg_config::Settings::fallback(root.clone());
    settings.import_dirs = vec![root.join("library"), root.join("test/syntax")];
    settings.base_library = Some(root.join("library/mini-base.k"));
    settings.target_device = "mock".to_owned();
    settings
}

fn print_table(corpus: &[(String, String)], stages: &[Stage]) {
    let total: usize = corpus.iter().map(|(_, t)| t.len()).sum();
    println!(
        "{} files, {:.1} MiB, median of {PASSES} passes\n",
        corpus.len(),
        total as f64 / 1_048_576.0
    );
    println!("{:<9} {:>7} {:>10} {:>10}   worst file", "stage", "files", "MiB/s", "worst");
    println!("{}", "-".repeat(72));
    for s in stages {
        println!(
            "{:<9} {:>7} {:>10.1} {:>9.2}ms   {}",
            s.name,
            s.files,
            s.mib_per_s(),
            s.worst.0.as_secs_f64() * 1000.0,
            s.worst.1
        );
    }

    // The number that decides whether this is usable as a language server.
    //
    // `keystroke` when it ran, `parse` only as a fallback. Reporting `parse` as
    // "the editor-latency ceiling" was true while the server published parse
    // errors and became a comfortable fiction the moment it started publishing
    // frontend ones — the parse is now a small fraction of what the user waits
    // for, and quoting it would have hidden a 20x gap.
    let headline = stages
        .iter()
        .find(|s| s.name == "keystroke")
        .or_else(|| stages.iter().find(|s| s.name == "parse"));
    if let Some(stage) = headline {
        let ms = stage.worst.0.as_secs_f64() * 1000.0;
        let verdict = if ms < 16.0 { "within" } else { "OVER" };
        let what = match stage.name {
            "keystroke" => {
                "one edit re-checked against the whole program — what the editor waits for \
                 before its first answer; `fixpoint` is its last"
            }
            _ => "a full reparse; the frontend is not in this number",
        };
        println!("\nworst case is {ms:.2} ms — {verdict} a 16 ms frame.\n{what}.");
        if stage.name == "keystroke" && ms >= 16.0 {
            println!(
                "Both halves are still whole-file: the parser has no incremental\n\
                 reparse, and `Db::diagnostics` re-runs every pass over the edited file."
            );
        }
    }
}

fn print_json(corpus: &[(String, String)], stages: &[Stage]) {
    let total: usize = corpus.iter().map(|(_, t)| t.len()).sum();
    let out = serde_json::json!({
        "corpus_files": corpus.len(),
        "corpus_bytes": total,
        "passes": PASSES,
        "stages": stages.iter().map(|s| serde_json::json!({
            "stage": s.name,
            "files": s.files,
            "bytes": s.bytes,
            "median_total_ms": s.total.as_secs_f64() * 1000.0,
            "mib_per_s": s.mib_per_s(),
            "worst_file": s.worst.1,
            "worst_ms": s.worst.0.as_secs_f64() * 1000.0,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("serializable"));
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
