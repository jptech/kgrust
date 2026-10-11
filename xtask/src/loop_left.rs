//! `cargo xtask loop-left`: what the program-wide loop leaves undecided, and
//! how many rounds it takes, root by root against the reference's own loop
//! (`docs/plans/sprint-parity.md` W0.1, W0.2, W0.6).
//!
//! **The settings are the recorded compile's.** Each root is elaborated as
//! `ptdiff` and `backend` compile it — `--import-dir library`, the default
//! base library, `mock` (`crate::ptrace::programs`) — never under
//! `bench::rooted_settings`, whose `mini-base.k` the reference's programs do
//! not compile against: `risc-v.k` counts 1039 instances left there and is
//! rejected with 16 errors, and 141 under its own flags (2026-09-27).
//!
//! **`left` is compared, not counted.** The reference leaves instances too:
//! a `TemplateInstance` the census never decides because nothing reaches it
//! outside a template (`Template.hs:88-100`) survives pass 8 in its tree as
//! in ours — `data/cache.k`'s seven are the reference's seven. So each side's
//! leftovers are spelled alike — `SCOPE|QUALIFIER|NAME`, the base's
//! `getScope`, `getQualifier` and `getName`, by `kg-oracle --loop` for the
//! reference and `kg_sem::elaborate::left_labels` here — and compared as
//! multisets. An instance only we leave is a decision the reference made and
//! we did not (W1.2's work); one only the reference leaves is a decision we
//! made that it did not; one both leave is not a gap. A root the reference
//! rejects (its recorded exit) is booked and not graded.
//!
//! **Rounds are the reference's own count**: the smallest
//! `--template-iterations R` whose tree equals the fixpoint's (the driver's
//! `loop`, `oracle/driver/Main.hs`), against `Report::rounds`. A root within
//! two rounds of `ROUND_GUARD`, or stopped by it, is `near-guard`.
//!
//! The reference's side is replayed from `crate::oracle_cache`; the first run
//! after the oracle or the reference changes searches every root's rounds
//! afresh — about ten frontends a root — and takes minutes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::conformance::settings_for;
use crate::ptrace::{programs, Loaded, Program};

/// What one side made of one root.
#[derive(Clone, Debug, PartialEq)]
struct Side {
    rounds: Option<u32>,
    /// Sorted.
    left: Vec<String>,
}

/// Ours, with whether the loop stopped at its guard.
struct Ours {
    side: Side,
    exhausted: bool,
}

pub fn run(kanagawa_root: &Path, filter: Option<&str>, verbose: bool) -> Result<()> {
    let kanagawa_root =
        &kanagawa_root.canonicalize().with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let Some(oracle) = crate::ir::find_oracle() else {
        anyhow::bail!("no kg-oracle under oracle/dist-newstyle — run oracle/build.sh; this gate grades against it");
    };
    let started = std::time::Instant::now();
    let mut work: Vec<Program> = Vec::new();
    let (mut rejected, mut unrecorded, mut stale) = (Vec::new(), 0usize, 0usize);
    for p in programs(kanagawa_root)? {
        if !p.name.starts_with("file::") || !filter.is_none_or(|f| p.name.contains(f)) {
            continue;
        }
        match crate::ptrace::load_run(kanagawa_root, &p)? {
            Loaded::Fresh(run, _) if run.exit == Some(0) => work.push(p),
            Loaded::Fresh(..) => rejected.push(p.name),
            Loaded::Stale => stale += 1,
            Loaded::Unrecorded => unrecorded += 1,
        }
    }
    if work.is_empty() {
        anyhow::bail!("nothing to grade ({unrecorded} roots unrecorded, {stale} stale) — run `cargo xtask ptrace` first");
    }

    // Ours: a `Db` per thread, as every corpus gate (`CLAUDE.md`).
    let ours = fan_out(&work, 8, |db: &mut Option<(kg_db::Db, Vec<String>)>, p| {
        let (db, current) = db.get_or_insert_with(|| (kg_db::Db::new(settings_for(kanagawa_root, &p.options)), p.options.clone()));
        if *current != p.options {
            db.set_settings(settings_for(kanagawa_root, &p.options));
            *current = p.options.clone();
        }
        let path = camino::Utf8PathBuf::from_path_buf(p.cwd.join(&p.file_arg)).ok()?;
        let root = db.file_id(&path);
        let looped = db.looped(&[root]);
        let report = &looped.out.report;
        Some(Ours {
            side: Side { rounds: Some(report.rounds), left: kg_sem::elaborate::left_labels(&looped.out, db.interner()) },
            exhausted: report.budget_exhausted,
        })
    });
    let ours_done = started.elapsed().as_secs_f64();

    // Theirs: one process each, holding two whole-program trees at once —
    // 0.1–0.4 GB for most roots; searched cold, about ten frontends a root.
    let library = kanagawa_root.join("library").display().to_string();
    let theirs = fan_out(&work, 10, |_: &mut Option<()>, p| {
        let path = p.cwd.join(&p.file_arg);
        let text = std::fs::read_to_string(&path).ok()?;
        let args = vec!["-I".to_owned(), library.clone(), "--loop".to_owned(), path.display().to_string()];
        let out = crate::oracle_cache::run(&oracle, kanagawa_root, &args, None, &text).ok()?;
        parse_oracle(&out.stdout)
    });

    let mut ledger = crate::ratchet::Ledger::new("loop-left");
    for set in ["agree", "ours-only", "theirs-only", "rounds-agree", "near-guard", "failed"] {
        ledger.set(set);
    }
    let guard = kg_sem::elaborate::ROUND_GUARD;
    let (mut graded, mut agree, mut rounds_agree) = (0usize, 0usize, 0usize);
    let (mut ours_left, mut theirs_left, mut shared_left) = (0usize, 0usize, 0usize);
    let mut ours_only_roots = 0usize;
    let mut lines = Vec::new();
    for ((p, o), t) in work.iter().zip(ours).zip(theirs) {
        let (Some(o), Some(t)) = (o, t) else {
            ledger.add("failed", &p.name);
            lines.push(format!("  FAILED {}", p.name));
            continue;
        };
        graded += 1;
        let (only_ours, only_theirs, shared) = multiset_diff(&o.side.left, &t.left);
        ours_left += o.side.left.len();
        theirs_left += t.left.len();
        shared_left += shared;
        if only_ours.is_empty() && only_theirs.is_empty() {
            agree += 1;
            ledger.add("agree", &p.name);
        }
        if !only_ours.is_empty() {
            ours_only_roots += 1;
        }
        for item in &only_ours {
            ledger.add("ours-only", &format!("{} :: {item}", p.name));
        }
        for item in &only_theirs {
            ledger.add("theirs-only", &format!("{} :: {item}", p.name));
        }
        if o.side.rounds == t.rounds {
            rounds_agree += 1;
            ledger.add("rounds-agree", &p.name);
        }
        let r = o.side.rounds.unwrap_or(0);
        if o.exhausted || r + 2 >= guard {
            ledger.add("near-guard", &p.name);
        }
        let delta = match (o.side.rounds, t.rounds) {
            (Some(a), Some(b)) => format!("{:+}", i64::from(a) - i64::from(b)),
            _ => "theirs > 64".to_owned(),
        };
        ledger.histogram("rounds, ours - theirs", &delta);
        if verbose || !only_ours.is_empty() || !only_theirs.is_empty() || o.side.rounds != t.rounds {
            lines.push(format!(
                "  {} left ours {} theirs {} (ours only {}, theirs only {}); rounds ours {} theirs {}{}",
                p.name,
                o.side.left.len(),
                t.left.len(),
                only_ours.len(),
                only_theirs.len(),
                show(o.side.rounds),
                show(t.rounds),
                if o.exhausted { ", AT THE GUARD" } else { "" }
            ));
            if verbose {
                for item in &only_ours {
                    lines.push(format!("      ours only    {item}"));
                }
                for item in &only_theirs {
                    lines.push(format!("      theirs only  {item}"));
                }
            }
        }
    }
    ledger.total("graded", graded);
    ledger.total("reference-rejected", rejected.len());
    ledger.total("ours-left", ours_left);
    ledger.total("theirs-left", theirs_left);
    ledger.total("shared-left", shared_left);
    ledger.total("ours-only-roots", ours_only_roots);

    for l in &lines {
        println!("{l}");
    }
    println!(
        "loop-left: {graded} roots the reference compiles, graded in {:.1}s (ours {ours_done:.1}s); {} it rejects, not graded; {unrecorded} unrecorded, {stale} stale",
        started.elapsed().as_secs_f64(),
        rejected.len()
    );
    println!("  left agree        {agree}/{graded} roots");
    println!(
        "  left              ours {ours_left}, the reference's {theirs_left}, both {shared_left}; ours only {} over {ours_only_roots} roots, theirs only {}",
        ledger.section.sets["ours-only"].len(),
        ledger.section.sets["theirs-only"].len()
    );
    println!("  rounds agree      {rounds_agree}/{graded} roots");
    println!("  near the guard    {} (within 2 of {guard}, or stopped by it)", ledger.section.sets["near-guard"].len());
    println!("  failed            {}", ledger.section.sets["failed"].len());
    let mut deltas: Vec<(i64, usize)> = ledger
        .section
        .histograms
        .get("rounds, ours - theirs")
        .into_iter()
        .flatten()
        .filter_map(|(k, &n)| Some((k.parse::<i64>().ok()?, n)))
        .collect();
    deltas.sort();
    println!(
        "  rounds, ours - theirs: {}",
        deltas.iter().map(|(d, n)| format!("{d:+} ×{n}")).collect::<Vec<_>>().join(", ")
    );
    println!("{}", crate::oracle_cache::report());
    if filter.is_none() {
        crate::ratchet::enforce(ledger)?;
    }
    Ok(())
}

fn show(r: Option<u32>) -> String {
    r.map_or_else(|| "> 64".to_owned(), |r| r.to_string())
}

/// `kg-oracle --loop`'s answer: `None` when it rejected the program.
fn parse_oracle(stdout: &str) -> Option<Side> {
    let mut left = Vec::new();
    let mut rounds = None;
    let mut saw_rounds = false;
    for line in stdout.lines() {
        if let Some(l) = line.strip_prefix("left ") {
            left.push(l.to_owned());
        } else if let Some(r) = line.strip_prefix("rounds ") {
            saw_rounds = true;
            rounds = r.trim().parse().ok();
        }
    }
    left.sort();
    saw_rounds.then_some(Side { rounds, left })
}

/// What only `ours` holds, what only `theirs` holds — each item numbered by
/// its occurrence (`label #k`), so a multiset is a set of names — and how
/// many both hold. Both sorted.
fn multiset_diff(ours: &[String], theirs: &[String]) -> (Vec<String>, Vec<String>, usize) {
    fn count(xs: &[String]) -> BTreeMap<&str, usize> {
        let mut m: BTreeMap<&str, usize> = BTreeMap::new();
        for x in xs {
            *m.entry(x.as_str()).or_default() += 1;
        }
        m
    }
    let (a, b) = (count(ours), count(theirs));
    let (mut only_a, mut only_b, mut shared) = (Vec::new(), Vec::new(), 0);
    for (k, &n) in &a {
        let m = b.get(k).copied().unwrap_or(0);
        shared += n.min(m);
        only_a.extend((m..n).map(|i| format!("{k} #{i}")));
    }
    for (k, &m) in &b {
        let n = a.get(k).copied().unwrap_or(0);
        only_b.extend((n..m).map(|i| format!("{k} #{i}")));
    }
    (only_a, only_b, shared)
}

/// `f` over every program on `workers` threads, each with its own state,
/// results in corpus order; a panic is that program's `None`.
fn fan_out<S, T: Send>(
    work: &[Program],
    workers: usize,
    f: impl Fn(&mut Option<S>, &Program) -> Option<T> + Sync,
) -> Vec<Option<T>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(workers);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<T>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut state: Option<S> = None;
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(p) = work.get(i) else { break };
                    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut state, p))).ok().flatten();
                    *slots[i].lock().expect("no panics hold this lock") = out;
                }
            });
        }
    });
    slots.into_iter().map(|s| s.into_inner().expect("threads joined")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = xs.iter().map(|s| (*s).to_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_leftover_both_sides_hold_is_shared_and_a_repeat_is_counted() {
        let (a, b, shared) = multiset_diff(&v(&["x", "x", "y"]), &v(&["x", "z"]));
        assert_eq!(a, ["x #1", "y #0"]);
        assert_eq!(b, ["z #0"]);
        assert_eq!(shared, 1);
    }

    #[test]
    fn the_oracles_answer_is_read_and_a_rejection_is_none() {
        let s = parse_oracle("left b|q|n\nleft a||m\nrounds 16\n").expect("answered");
        assert_eq!(s, Side { rounds: Some(16), left: v(&["a||m", "b|q|n"]) });
        assert_eq!(parse_oracle("rounds >64\n").expect("answered").rounds, None);
        assert_eq!(parse_oracle(""), None);
    }
}
