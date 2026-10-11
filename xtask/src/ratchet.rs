//! Per-item ratchets: every gate's verdict, item by item, against the one
//! checked in at `baseline.json` (AUDIT §6.3, §7).
//!
//! A total that rises can hide a program that regressed: `891fc8c` moved
//! `ptdiff` 1735 → 1742 as "9 up, 2 down", and the two were visible only to
//! someone who diffed two `--list` runs by hand. So each ratcheted gate —
//! `strict-ledger` (conformance and `--strict`, per assertion), `errors` (per
//! snippet), `ptdiff` and `backend` (per program), `repoint` (per registered
//! test) — ends an unfiltered run by writing a **ledger**,
//! `target/gates/<gate>.json`: its totals, its named sets of items, and its
//! histograms. The ledger is compared with the gate's section of
//! `baseline.json`, and the run fails on any item that got worse:
//!
//! * a **pass set** (`ptdiff`'s `structural` and `exact`, `backend`'s
//!   `agree`, `repoint`'s `pass`) fails on an item that *left* it — which
//!   includes an item that was not graded at all this time, so an
//!   unrecorded or stale program is never a silent pass;
//! * a **fail set** (`strict`'s `failing`, `errors`' `disagree` and
//!   `silent`, `backend`'s `crash`, …) fails on an item that *entered* it;
//! * a total named `graded` may not fall, which is what keeps a fail set
//!   honest when the items it would have held are simply not graded.
//!
//! Improvements are printed and do not fail: lock them in with
//! `cargo xtask ratchet --bless <gate>`, in the same commit as the change
//! that made them — until then a later regression of the same item is
//! invisible. The same command, deliberately, is how a regression is
//! accepted; the baseline's diff in that commit names every item it gave up.
//!
//! Histograms (decline reasons, first divergences) are reported as deltas
//! and never fail a run: a decline turning into a disagreement moves an item
//! between two failing classes, and is shown as exactly that.
//!
//! Each ledger carries the fingerprint of the tree it was graded from
//! (`crate::fresh::tree_fingerprint`); `ratchet` refuses to read or bless a
//! ledger from another tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Every ratcheted gate, in the order the summary prints them.
pub(crate) const GATES: &[&str] =
    &["strict", "errors", "ptdiff", "backend", "output", "repoint", "cli", "loop-left", "lint", "resilience", "shadow"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Polarity {
    /// Leaving the set is a regression.
    Pass,
    /// Entering the set is a regression.
    Fail,
}

/// Which way each set points. A set not named here is a bug in the gate
/// that wrote it, and `compare` says so rather than guessing.
fn polarity(gate: &str, set: &str) -> Option<Polarity> {
    use Polarity::{Fail, Pass};
    Some(match (gate, set) {
        ("strict", "failing" | "conformance-failing") => Fail,
        ("errors", "disagree" | "silent" | "oracle-silent" | "skipped") => Fail,
        ("ptdiff", "exact" | "structural") => Pass,
        ("ptdiff", "rejected" | "failed") => Fail,
        ("backend", "agree") => Pass,
        ("backend", "crash") => Fail,
        // Rejected alike and the first error line the same, file name,
        // line and message (sprint W6).
        ("backend", "message") => Pass,
        ("output", "equal" | "exact") => Pass,
        ("repoint", "pass") => Pass,
        // `crate::cli`: upstream's CLI tests and the help, by name.
        ("cli", "pass") => Pass,
        ("loop-left", "agree" | "rounds-agree") => Pass,
        ("loop-left", "ours-only" | "theirs-only" | "near-guard" | "failed") => Fail,
        // `crate::resilience`: every answer the editor gives. Until sprint
        // W3.7 the loop's against the CST engine's: `cst`, `agree`,
        // `copies` passing, `disagree`, `ir-only` failing.
        ("resilience", "answered") => Pass,
        ("resilience", "failed") => Fail,
        // Every set is a rule's occurrences (`crate::lint`).
        ("lint", _) => Fail,
        // `edit-identity`'s snapshot shadow (sprint W7.5d): the steps whose
        // every followed round's key agreed, and the modules whose restored
        // state is not the round run.
        ("shadow", "kept") => Pass,
        ("shadow", "unequal") => Fail,
        // A prefix pass the editor took from its kept run that the pass,
        // run on a fork, does not make (D1 step 3, `kg_sem::reuse`).
        ("shadow", "reuse-unequal") => Fail,
        _ => return None,
    })
}

/// What a gate graded, item by item.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Section {
    #[serde(default)]
    pub totals: BTreeMap<String, usize>,
    #[serde(default)]
    pub sets: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub histograms: BTreeMap<String, BTreeMap<String, usize>>,
}

/// A gate's ledger: its section, and the tree it was graded from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Ledger {
    pub gate: String,
    pub fingerprint: String,
    #[serde(flatten)]
    pub section: Section,
}

impl Ledger {
    pub(crate) fn new(gate: &str) -> Ledger {
        Ledger { gate: gate.to_owned(), fingerprint: String::new(), section: Section::default() }
    }

    pub(crate) fn total(&mut self, name: &str, n: usize) {
        self.section.totals.insert(name.to_owned(), n);
    }

    /// Declare a set, so an empty one is still written (and compared).
    pub(crate) fn set(&mut self, name: &str) -> &mut BTreeSet<String> {
        self.section.sets.entry(name.to_owned()).or_default()
    }

    pub(crate) fn add(&mut self, set: &str, item: &str) {
        self.set(set).insert(item.to_owned());
    }

    pub(crate) fn histogram(&mut self, name: &str, key: &str) {
        *self.section.histograms.entry(name.to_owned()).or_default().entry(key.to_owned()).or_default() += 1;
    }

    /// A histogram entry set to `n` — a per-item count the comparison names
    /// when it moves, without grading it.
    pub(crate) fn histogram_count(&mut self, name: &str, key: &str, n: usize) {
        self.section.histograms.entry(name.to_owned()).or_default().insert(key.to_owned(), n);
    }
}

/// The checked-in file: one section per gate.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Baseline {
    #[serde(rename = "_about", default)]
    about: String,
    #[serde(default)]
    gates: BTreeMap<String, Section>,
}

const ABOUT: &str = "Per-item ratchets for the xtask gates (xtask/src/ratchet.rs). \
Regenerate a section only deliberately: `cargo xtask ratchet --bless <gate>` after an unfiltered gate run.";

/// `baseline.json` — or `KG_RATCHET_BASELINE`'s file, for proving a new
/// gate's mutants red against the baseline it is about to be blessed with,
/// before the blessing (`scripts/mutant.sh`); never for a gate run.
fn baseline_path() -> PathBuf {
    std::env::var_os("KG_RATCHET_BASELINE").map_or_else(|| crate::fresh::workspace().join("baseline.json"), PathBuf::from)
}

pub(crate) fn gates_dir() -> PathBuf {
    crate::fresh::workspace().join("target/gates")
}

fn ledger_path(gate: &str) -> PathBuf {
    gates_dir().join(format!("{gate}.json"))
}

fn read_baseline() -> Result<Baseline> {
    let path = baseline_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Baseline::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json(path: &PathBuf, value: &impl Serialize) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Every item of every set in `baseline.json` that `map` names, renamed —
/// for `crate::rebase`. An item mapped to `None` is kept as it was and
/// returned, with its gate, for the report. Returns how many were renamed.
pub(crate) fn rename_items(map: &BTreeMap<String, Option<String>>) -> Result<(usize, Vec<(String, String)>)> {
    let mut baseline = read_baseline()?;
    let mut renamed = 0usize;
    let mut unmapped = Vec::new();
    for (gate, section) in &mut baseline.gates {
        for set in section.sets.values_mut() {
            let items = std::mem::take(set);
            for item in items {
                match map.get(&item) {
                    Some(Some(new)) => {
                        renamed += usize::from(*new != item);
                        set.insert(new.clone());
                    }
                    Some(None) => {
                        unmapped.push((gate.clone(), item.clone()));
                        set.insert(item);
                    }
                    None => {
                        set.insert(item);
                    }
                }
            }
        }
    }
    write_json(&baseline_path(), &baseline)?;
    Ok((renamed, unmapped))
}

/// What one comparison found.
#[derive(Default)]
struct Delta {
    regressions: usize,
    improvements: usize,
    /// One line per set and total, for the summary.
    headline: Vec<String>,
    /// Every newly failing item, and a sample of newly passing ones.
    detail: Vec<String>,
}

fn compare(gate: &str, base: &Section, now: &Section) -> Delta {
    let mut d = Delta::default();
    let names: BTreeSet<&String> = base.sets.keys().chain(now.sets.keys()).collect();
    let empty = BTreeSet::new();
    for name in names {
        let Some(pol) = polarity(gate, name) else {
            d.regressions += 1;
            d.detail.push(format!("    set `{name}` has no polarity in ratchet.rs — the gate and the ratchet disagree"));
            continue;
        };
        let b = base.sets.get(name).unwrap_or(&empty);
        let n = now.sets.get(name).unwrap_or(&empty);
        let (worse, better): (Vec<&String>, Vec<&String>) = match pol {
            Polarity::Pass => (b.difference(n).collect(), n.difference(b).collect()),
            Polarity::Fail => (n.difference(b).collect(), b.difference(n).collect()),
        };
        d.regressions += worse.len();
        d.improvements += better.len();
        let kind = if pol == Polarity::Pass { "pass" } else { "fail" };
        d.headline.push(format!(
            "{name} ({kind} set) {} → {}{}",
            b.len(),
            n.len(),
            if worse.is_empty() && better.is_empty() {
                String::new()
            } else {
                format!("   {} newly passing, {} newly failing", better.len(), worse.len())
            }
        ));
        for item in &worse {
            d.detail.push(format!("    NEWLY FAILING  {name}: {item}"));
        }
        for item in better.iter().take(10) {
            d.detail.push(format!("    newly passing  {name}: {item}"));
        }
        if better.len() > 10 {
            d.detail.push(format!("    … and {} more newly passing in {name}", better.len() - 10));
        }
    }
    if let (Some(&b), Some(&n)) = (base.totals.get("graded"), now.totals.get("graded"))
        && n < b
    {
        d.regressions += 1;
        d.detail.push(format!("    GRADED FEWER  {b} → {n} items — an item not graded is not a pass"));
    }
    let moved: Vec<String> = now
        .totals
        .iter()
        .filter_map(|(k, &v)| {
            let was = base.totals.get(k).copied();
            (was != Some(v)).then(|| match was {
                Some(w) => format!("{k} {w}→{v}"),
                None => format!("{k} (new) {v}"),
            })
        })
        .collect();
    if !moved.is_empty() {
        d.headline.push(format!("totals moved: {}", moved.join(", ")));
    }
    for (h, now_h) in &now.histograms {
        let base_h = base.histograms.get(h).cloned().unwrap_or_default();
        let keys: BTreeSet<&String> = base_h.keys().chain(now_h.keys()).collect();
        for k in keys {
            let (was, is) = (base_h.get(k).copied().unwrap_or(0), now_h.get(k).copied().unwrap_or(0));
            if was != is {
                d.detail.push(format!("    {h}: {k}  {was} → {is}"));
            }
        }
    }
    d
}

fn print_delta(gate: &str, d: &Delta, blessing: bool) {
    println!("ratchet {gate} (against baseline.json):");
    for l in &d.headline {
        println!("  {l}");
    }
    for l in &d.detail {
        println!("{l}");
    }
    if !blessing && d.improvements > 0 && d.regressions == 0 {
        println!("  {} improvement(s) not yet in the baseline — `cargo xtask ratchet --bless {gate}` locks them in", d.improvements);
    }
}

/// End a gate's unfiltered run: write its ledger, compare it with the
/// baseline, fail on any regression. `Ok(true)` when the baseline had a
/// section for this gate and nothing regressed — the gate's verdict is then
/// the ratchet's, not its legacy "any failure at all" rule; `Ok(false)` when
/// there is no baseline to hold it to yet.
pub(crate) fn enforce(mut ledger: Ledger) -> Result<bool> {
    ledger.fingerprint = crate::fresh::tree_fingerprint();
    std::fs::create_dir_all(gates_dir())?;
    write_json(&ledger_path(&ledger.gate), &ledger)?;
    let baseline = read_baseline()?;
    let Some(base) = baseline.gates.get(&ledger.gate) else {
        println!(
            "ratchet {}: no section in baseline.json — nothing to hold this run to; `cargo xtask ratchet --bless {}` records one",
            ledger.gate, ledger.gate
        );
        return Ok(false);
    };
    let d = compare(&ledger.gate, base, &ledger.section);
    println!();
    print_delta(&ledger.gate, &d, false);
    if d.regressions > 0 {
        anyhow::bail!(
            "ratchet {}: {} regression(s) against baseline.json — fix them, or accept them deliberately with `cargo xtask ratchet --bless {}`",
            ledger.gate,
            d.regressions,
            ledger.gate
        );
    }
    Ok(true)
}

/// A sampled run's verdict (`--every`, `--filter`): `ledger` against the
/// baseline's items for the roots it `graded` — an item is `root|…` —
/// every other item and every total left out. Writes no ledger: a sample
/// is never blessed.
pub(crate) fn enforce_sample(ledger: &Ledger, graded: &BTreeSet<String>) -> Result<()> {
    let baseline = read_baseline()?;
    let Some(base) = baseline.gates.get(&ledger.gate) else {
        println!("ratchet {}: no section in baseline.json — nothing to hold this sample to", ledger.gate);
        return Ok(());
    };
    let ran = |item: &String| item.split('|').next().is_some_and(|root| graded.contains(root));
    let restrict = |s: &Section| Section {
        sets: s.sets.iter().map(|(k, v)| (k.clone(), v.iter().filter(|i| ran(i)).cloned().collect())).collect(),
        ..Section::default()
    };
    let d = compare(&ledger.gate, &restrict(base), &restrict(&ledger.section));
    println!();
    print_delta(&format!("{} (a sample: {} roots)", ledger.gate, graded.len()), &d, true);
    if d.regressions > 0 {
        anyhow::bail!("ratchet {}: {} regression(s) against baseline.json on the roots sampled", ledger.gate, d.regressions);
    }
    Ok(())
}

/// The latest ledger for a gate, if it was graded from this tree.
fn read_ledger(gate: &str, fingerprint: &str) -> std::result::Result<Ledger, String> {
    let path = ledger_path(gate);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("no ledger at {} — run the gate unfiltered first", path.display()))?;
    let ledger: Ledger = serde_json::from_str(&text).map_err(|e| format!("unreadable ledger {}: {e}", path.display()))?;
    if ledger.fingerprint != fingerprint {
        return Err(format!("STALE ledger {} — graded from a different tree; re-run the gate", path.display()));
    }
    Ok(ledger)
}

/// One stage's outcome, as `scripts/gates.sh` reports it: `name=ok` or
/// `name=FAIL`, with an optional `:note`.
fn parse_stage(s: &str) -> (String, bool, String) {
    let (name, rest) = s.split_once('=').unwrap_or((s, "FAIL"));
    let (status, note) = rest.split_once(':').unwrap_or((rest, ""));
    (name.to_owned(), status == "ok", note.to_owned())
}

/// `cargo xtask ratchet`: re-read every ledger, compare, print one screen,
/// write `target/gates/status.json`; or, with `--bless`, write the named
/// gates' ledgers into `baseline.json`.
pub fn run(gates: &[String], bless: bool, stages: &[String]) -> Result<()> {
    let wanted: Vec<String> =
        if gates.is_empty() { GATES.iter().map(|g| (*g).to_owned()).collect() } else { gates.to_vec() };
    for g in &wanted {
        if !GATES.contains(&g.as_str()) {
            anyhow::bail!("no ratcheted gate named `{g}` — one of {}", GATES.join(", "));
        }
    }
    let fingerprint = crate::fresh::tree_fingerprint();
    let mut baseline = read_baseline()?;

    if bless {
        let mut problems = Vec::new();
        for g in &wanted {
            match read_ledger(g, &fingerprint) {
                Ok(ledger) => {
                    let base = baseline.gates.get(g).cloned().unwrap_or_default();
                    let d = compare(g, &base, &ledger.section);
                    print_delta(g, &d, true);
                    if d.regressions > 0 {
                        println!("  BLESSING {} regression(s) in {g}: the items above leave the baseline", d.regressions);
                    }
                    baseline.gates.insert(g.clone(), ledger.section);
                }
                Err(e) => problems.push(format!("{g}: {e}")),
            }
        }
        if !problems.is_empty() {
            anyhow::bail!("nothing blessed:\n  {}", problems.join("\n  "));
        }
        baseline.about = ABOUT.to_owned();
        write_json(&baseline_path(), &baseline)?;
        println!("blessed {} into {}", wanted.join(", "), baseline_path().display());
        return Ok(());
    }

    let mut out = String::new();
    let mut failed = false;
    let mut status_gates = serde_json::Map::new();
    let stages: Vec<(String, bool, String)> = stages.iter().map(|s| parse_stage(s)).collect();
    if !stages.is_empty() {
        let _ = writeln!(out, "stages:");
        for (name, ok, note) in &stages {
            failed |= !ok;
            let _ = writeln!(out, "  {:<5} {name:<14} {note}", if *ok { "ok" } else { "FAIL" });
        }
    }
    let _ = writeln!(out, "ratchets (baseline.json):");
    let mut details = String::new();
    for g in &wanted {
        let entry = match read_ledger(g, &fingerprint) {
            Err(e) => {
                failed = true;
                let _ = writeln!(out, "  FAIL  {g:<14} {e}");
                serde_json::json!({ "error": e })
            }
            Ok(ledger) => {
                let totals = serde_json::to_value(&ledger.section.totals)?;
                match baseline.gates.get(g) {
                    None => {
                        failed = true;
                        let _ = writeln!(out, "  FAIL  {g:<14} no baseline section — `cargo xtask ratchet --bless {g}`");
                        serde_json::json!({ "totals": totals, "baseline": false })
                    }
                    Some(base) => {
                        let d = compare(g, base, &ledger.section);
                        failed |= d.regressions > 0;
                        let sizes: Vec<String> =
                            ledger.section.sets.iter().map(|(k, v)| format!("{k} {}", v.len())).collect();
                        let _ = writeln!(
                            out,
                            "  {:<5} {g:<14} {}   (+{} / -{} against baseline)",
                            if d.regressions > 0 { "FAIL" } else { "ok" },
                            sizes.join(", "),
                            d.improvements,
                            d.regressions
                        );
                        if d.regressions > 0 {
                            for l in d.detail.iter().filter(|l| l.contains("NEWLY FAILING") || l.contains("GRADED FEWER")) {
                                let _ = writeln!(details, "{l}");
                            }
                        }
                        let sets: serde_json::Map<String, serde_json::Value> =
                            ledger.section.sets.iter().map(|(k, v)| (k.clone(), v.len().into())).collect();
                        serde_json::json!({
                            "totals": totals,
                            "sets": sets,
                            "regressions": d.regressions,
                            "improvements": d.improvements,
                        })
                    }
                }
            }
        };
        status_gates.insert(g.clone(), entry);
    }
    print!("{out}{details}");
    let status = serde_json::json!({
        "generated_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "head": git_head(),
        "fingerprint": fingerprint,
        "ok": !failed,
        "stages": stages.iter().map(|(n, ok, note)| (n.clone(), serde_json::json!({ "ok": ok, "note": note }))).collect::<serde_json::Map<_, _>>(),
        "gates": status_gates,
    });
    std::fs::create_dir_all(gates_dir())?;
    write_json(&gates_dir().join("status.json"), &status)?;
    if failed {
        anyhow::bail!("the gates are red — see above (target/gates/status.json)");
    }
    println!("all green (target/gates/status.json)");
    Ok(())
}

fn git_head() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(crate::fresh::workspace())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-item count that moved is named in the detail — which item,
    /// from what to what — and graded neither way: `output`'s location
    /// tally per program. Red with histograms left out of the comparison.
    #[test]
    fn a_moved_histogram_count_is_named_not_graded() {
        let mut base = Ledger::new("output");
        base.histogram_count("locations-agree", "file::a.k", 120);
        base.histogram_count("locations-agree", "file::b.k", 7);
        let mut now = Ledger::new("output");
        now.histogram_count("locations-agree", "file::a.k", 118);
        now.histogram_count("locations-agree", "file::b.k", 7);
        let d = compare("output", &base.section, &now.section);
        assert_eq!((d.regressions, d.improvements), (0, 0));
        assert_eq!(d.detail, ["    locations-agree: file::a.k  120 → 118"]);
    }

    fn section(sets: &[(&str, &[&str])], graded: usize) -> Section {
        let mut s = Section::default();
        s.totals.insert("graded".into(), graded);
        for (name, items) in sets {
            s.sets.insert((*name).to_owned(), items.iter().map(|i| (*i).to_owned()).collect());
        }
        s
    }

    #[test]
    fn a_rising_total_still_fails_on_the_item_that_regressed() {
        // "9 up, 2 down": the pass set grows and the ratchet still fails.
        let base = section(&[("structural", &["a", "b", "c"])], 10);
        let now = section(&[("structural", &["a", "c", "d", "e"])], 10);
        let d = compare("ptdiff", &base, &now);
        assert_eq!((d.regressions, d.improvements), (1, 2));
        assert!(d.detail.iter().any(|l| l.contains("NEWLY FAILING") && l.ends_with(": b")));
    }

    #[test]
    fn a_fail_set_fails_on_entry_and_on_grading_less() {
        let base = section(&[("failing", &["x"])], 10);
        assert_eq!(compare("strict", &base, &section(&[("failing", &["x", "y"])], 10)).regressions, 1);
        assert_eq!(compare("strict", &base, &section(&[("failing", &[])], 10)).regressions, 0);
        // Emptying the fail set by grading nothing is caught by `graded`.
        assert_eq!(compare("strict", &base, &section(&[("failing", &[])], 3)).regressions, 1);
    }

    #[test]
    fn an_ungraded_item_leaves_a_pass_set() {
        let base = section(&[("agree", &["p", "q"])], 2);
        let now = section(&[("agree", &["p"])], 2);
        assert_eq!(compare("backend", &base, &now).regressions, 1);
    }

    #[test]
    fn an_unknown_set_is_an_error_not_a_guess() {
        let now = section(&[("mystery", &["a"])], 1);
        assert_eq!(compare("ptdiff", &Section::default(), &now).regressions, 1);
    }
}
