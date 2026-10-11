//! `cargo xtask strict-ledger` — `conformance` and `conformance --strict`,
//! item by item, for the ratchet.
//!
//! `conformance.rs` is the scoreboard and nothing graded may edit it
//! (`AGENTS.md`), and its report names at most 40 failures — enough to read,
//! not enough to ratchet: "strict must never rise" was enforced by a person
//! comparing one number between two logs. So the per-assertion set is
//! computed here instead, through the scoreboard's own public pieces —
//! [`load_all`], [`settings_for`], [`snippet_path`], [`score`] and
//! [`Scoreboard`] — with the frontend reached through the same seam
//! `conformance.rs`'s private `check_snippet` uses, `Db::check_snippet`.
//! Each snippet is checked once and scored twice, as the scoreboard scores it
//! (`FRONTEND_COMPLETE`) and as `--strict` does — since the constant was
//! flipped (2026-10-10) the two readings agree, and both are kept so the
//! ledger still follows the scoreboard if it is ever set back.
//!
//! **This is a second reading of the scoreboard, and it is checked against
//! the first.** `--against <log>` takes a `conformance` or `conformance
//! --strict` log and fails unless its headline is byte-identical to the one
//! computed here and every `FAIL` it names is in this ledger's set;
//! `scripts/gates.sh` passes both logs. If `check_snippet` or `score` ever
//! changes in a way this module does not follow, that check goes red before
//! a ratchet reads a wrong set.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::conformance::{
    Assertion, CheckResult, FRONTEND_COMPLETE, Outcome, Scoreboard, load_all, score, settings_for, snippet_path,
};

/// What `conformance.rs`'s `check_snippet` computes, from the same call.
fn check(db: &mut kg_db::Db, path: &camino::Utf8Path, source: &str) -> CheckResult {
    let diagnostics = db.check_snippet(path, source);
    let first_error = diagnostics.iter().find(|d| d.is_error());
    CheckResult {
        first_error_code: u32::from(first_error.is_some()),
        first_error: first_error.map(|d| d.message.clone()),
        first_warning: diagnostics.iter().find_map(|d| d.warning.map(|w| w.name().to_owned())),
        internal_error: false,
    }
}

/// Every assertion's `(conformance, strict)` outcome, in corpus order — the
/// scoreboard's fan-out: consecutive chunks to threads that each own a `Db`.
fn outcomes(kanagawa_root: &Path, assertions: &[Assertion], jobs: usize) -> Vec<(Outcome, Outcome)> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let chunk = assertions.len().div_ceil(jobs * 4).max(1);
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<(Outcome, Outcome)>>> = assertions.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(settings_for(kanagawa_root, &[]));
                let mut current: Option<Arc<[String]>> = None;
                loop {
                    let start = next.fetch_add(chunk, Ordering::SeqCst);
                    if start >= assertions.len() {
                        break;
                    }
                    for (i, a) in assertions.iter().enumerate().skip(start).take(chunk) {
                        if current.as_deref() != Some(&*a.options) {
                            db.set_settings(settings_for(kanagawa_root, &a.options));
                            current = Some(Arc::clone(&a.options));
                        }
                        let result = check(&mut db, &snippet_path(kanagawa_root, a), &a.source);
                        *out[i].lock().expect("no panics hold this lock") =
                            Some((score(a, &result, FRONTEND_COMPLETE), score(a, &result, true)));
                    }
                }
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined").expect("every slot filled")).collect()
}

pub fn run(kanagawa_root: &Path, against: &[std::path::PathBuf], jobs: Option<usize>) -> Result<()> {
    if !kanagawa_root.join("test/syntax").is_dir() {
        anyhow::bail!("no reference corpus at {} — pass --kanagawa-root", kanagawa_root.display());
    }
    let assertions = load_all(kanagawa_root)?;
    let jobs = jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8))
        .max(1);
    let mut boards = (Scoreboard::default(), Scoreboard::default());
    let mut ledger = crate::ratchet::Ledger::new("strict");
    ledger.set("failing");
    ledger.set("conformance-failing");
    for (a, (plain, strict)) in assertions.iter().zip(outcomes(kanagawa_root, &assertions, jobs)) {
        if matches!(plain, Outcome::Fail(_)) {
            ledger.add("conformance-failing", &a.name);
        }
        if matches!(strict, Outcome::Fail(_)) {
            ledger.add("failing", &a.name);
        }
        boards.0.record(&a.name, plain);
        boards.1.record(&a.name, strict);
    }
    let headline = |b: &Scoreboard| b.report().lines().next().unwrap_or("").to_owned();
    println!("conformance: {}", headline(&boards.0));
    println!("strict:      {}", headline(&boards.1));
    ledger.total("graded", assertions.len());
    for (prefix, b) in [("conformance", &boards.0), ("strict", &boards.1)] {
        ledger.total(&format!("{prefix}-passed"), b.passed);
        ledger.total(&format!("{prefix}-failed"), b.failed);
        ledger.total(&format!("{prefix}-undecidable"), b.undecidable);
    }

    // The cross-check: each log is one of the two readings, told apart by
    // its headline.
    for log in against {
        let text = std::fs::read_to_string(log).with_context(|| format!("reading {}", log.display()))?;
        let theirs = text
            .lines()
            .find(|l| l.contains("decidable assertions pass"))
            .with_context(|| format!("{} has no scoreboard headline", log.display()))?;
        let (which, board, set) = if theirs == headline(&boards.1) {
            ("strict", &boards.1, "failing")
        } else if theirs == headline(&boards.0) {
            ("conformance", &boards.0, "conformance-failing")
        } else {
            anyhow::bail!(
                "{} reads `{theirs}`, and this ledger computed neither `{}` nor `{}` — the ledger no longer reads the scoreboard the way conformance.rs does",
                log.display(),
                headline(&boards.0),
                headline(&boards.1)
            );
        };
        let named: Vec<&str> =
            text.lines().filter_map(|l| l.strip_prefix("  FAIL ")).filter_map(|l| l.split(": ").next()).collect();
        let missing: Vec<&&str> = named.iter().filter(|n| !ledger.section.sets[set].contains(**n)).collect();
        if !missing.is_empty() {
            anyhow::bail!("{} names {} failure(s) this ledger does not, e.g. {}", log.display(), missing.len(), missing[0]);
        }
        println!("  agrees with {} ({which}: headline, and all {} FAIL lines it prints)", log.display(), named.len());
        debug_assert_eq!(board.failed, ledger.section.sets[set].len());
    }

    if !crate::ratchet::enforce(ledger)? {
        println!("(no baseline yet: nothing enforced)");
    }
    Ok(())
}
