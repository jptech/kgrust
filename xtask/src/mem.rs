//! `cargo xtask mem` — the frontend's peak memory per use case, held to
//! budgets.
//!
//! The owner accepted on 2026-10-10 that the editor keep a second copy of
//! a program between edits (sprint W7.5d, D1 — `docs/plans/loop-across-edits.md`
//! §6), on a condition: "keep an eye on memory usage across use cases", and
//! stay efficient enough that the copy is cheap. This is the eye. Until it
//! existed a peak was read off `/usr/bin/time -l` by hand when someone
//! thought to, and a doubling would have shown only in the gates' own
//! 17 GB (`bounded.rs`).
//!
//! Three use cases a program, each a fresh process, so no peak hides
//! another's:
//!
//! - **`check`** — `kg check`: [`kg_db::Db::check`] at the fixpoint
//!   (`crates/kg-cli/src/modern/mod.rs`, `check`).
//! - **`editor-open`** — the language server's check of a file just opened:
//!   [`kg_db::Db::diagnostics_layered`] under [`kg_db::PassBudget::EDITOR`],
//!   what `kg_lsp::GlobalState` calls, to the fixpoint, with the allocator
//!   purging as the server's does ([`kg_alloc::purge_eagerly`]).
//! - **`editor-edits`** — the same, then `--keystrokes` edits, each run to
//!   the fixpoint: what the editor holds between edits — memos, the loop's
//!   kept state — beside what one check needs. Its excess over `open` is
//!   what the edits kept.
//!
//! The children are this binary (`--child`), not `kg`: every program is
//! checked under its *recorded* compile settings (`ptrace::programs`,
//! `conformance::settings_for`) — the full base library, its defines —
//! the program the reference compiles, which `kg check`'s flags cannot all
//! say (`risc-v.k` under the bench's rooted settings is one the reference
//! rejects). The protocol is not measured: the server's own allocations
//! beside its `Db` are a document's text and a request's answer.
//!
//! The peak is read off `/usr/bin/time` — the instrument
//! `docs/plans/performance.md`'s figures were read with; xtask links no
//! `libc` (`bounded.rs`). On macOS it is the **peak footprint**, the
//! process's dirty and compressed pages, what Activity Monitor shows: the
//! resident set there also counts pages mimalloc has already purged, which
//! the kernel takes back only under pressure — `array.k` after twelve
//! edits holds 630 MiB resident, a 380 MB footprint and 133 MiB of live
//! heap (2026-10-10, `vmmap`, and a counting allocator). Elsewhere, where
//! a purge leaves the resident set, it is the resident set. Either
//! includes this binary's own ~10 MiB, the same in every row.
//!
//! **Budgets** are `memory-budgets.json`, at the workspace root beside
//! `baseline.json` so that blessing one does not stale the gates' ledgers
//! (`fresh::tree_fingerprint`). A peak over its budget fails the run;
//! `--bless` resets every measured budget to its peak plus a quarter. A
//! peak moves from run to run with the loop's threads and the allocator's
//! timing — `risc-v.k`'s `editor-edits` read 1391 to 1522 MiB over three
//! runs of one tree (2026-10-10, load 15–26) — and a quarter is above that
//! and below what a design change does: D1's copy of `risc-v.k` is
//! ~0.7 GiB on ~1.5.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::bounded;
use crate::fresh::workspace;

/// Smallest to largest, the rows of `performance.md`'s comparison with the
/// reference — and [`TINY`] before them, the base library alone.
const PROGRAMS: &[&str] = &[
    "test/library/data/cache.k",
    "test/library/data/array.k",
    "test/library/numeric/float.k",
    "test/library/processor/risc-v.k",
];

/// A four-line function: what the base library alone costs, the floor
/// under every program.
const TINY_NAME: &str = "tiny.k";
const TINY: &str = "inline uint32 add_one(uint32 a)\n{\n    return a + 1;\n}\n";

/// `TINY` runs under the first program's settings, without its defines.
const TINY_SETTINGS_FROM: &str = PROGRAMS[0];

const CASES: [&str; 3] = ["check", "editor-open", "editor-edits"];

/// The budget's headroom over a blessed peak.
const HEADROOM: f64 = 1.25;

fn budgets_path() -> PathBuf {
    workspace().join("memory-budgets.json")
}

/// What a budget holds: the footprint where `/usr/bin/time` reports one
/// (macOS), the resident set elsewhere. `memory-budgets.json` names it, and
/// a file blessed under the other is not graded.
const METRIC: &str = if cfg!(target_os = "macos") { "footprint" } else { "rss" };

/// One measured process.
struct Peak {
    rss_mib: u64,
    /// macOS only: what Activity Monitor calls the process's memory.
    footprint_mib: Option<u64>,
    secs: f64,
    errors: usize,
}

impl Peak {
    /// The [`METRIC`] reading.
    fn graded(&self) -> u64 {
        self.footprint_mib.unwrap_or(self.rss_mib)
    }
}

pub(crate) fn run(kanagawa_root: &Path, keystrokes: usize, filter: Option<&str>, bless: bool, json_out: bool) -> Result<()> {
    let root = kanagawa_root.canonicalize().with_context(|| format!("{}", kanagawa_root.display()))?;
    let programs = crate::ptrace::programs(&root)?;
    let options_of = |file: &str| -> Result<Vec<String>> {
        let p = programs
            .iter()
            .find(|p| p.name == format!("file::{file}"))
            .with_context(|| format!("no recorded compile of {file} in the corpus"))?;
        Ok(p.options.clone())
    };
    // Children run here, never under the reference (`guard`).
    let work = workspace().join("target/mem");
    std::fs::create_dir_all(&work)?;
    let tiny = work.join(TINY_NAME);
    std::fs::write(&tiny, TINY)?;

    let mut rows: Vec<(String, PathBuf, Vec<String>)> = Vec::new();
    let tiny_options: Vec<String> =
        options_of(TINY_SETTINGS_FROM)?.into_iter().filter(|o| !o.starts_with("--define")).collect();
    rows.push((TINY_NAME.to_owned(), tiny, tiny_options));
    for file in PROGRAMS {
        rows.push(((*file).to_owned(), root.join(file), options_of(file)?));
    }
    rows.retain(|(name, ..)| filter.is_none_or(|f| name.contains(f)));
    if rows.is_empty() {
        bail!("no program matched --filter");
    }

    let exe = std::env::current_exe()?;
    let mut measured: Vec<(String, [Peak; 3])> = Vec::new();
    for (name, path, options) in &rows {
        let mut peaks = Vec::with_capacity(3);
        for case in CASES {
            let mut cmd = Command::new("/usr/bin/time");
            cmd.arg(if cfg!(target_os = "macos") { "-l" } else { "-v" })
                .arg(&exe)
                .arg("mem")
                .arg("--kanagawa-root")
                .arg(&root)
                .arg("--keystrokes")
                .arg(keystrokes.to_string())
                .arg("--child")
                .arg(path)
                .arg("--case")
                .arg(case)
                .arg("--")
                .args(options)
                .current_dir(&work);
            let t0 = Instant::now();
            let output = match bounded::run(cmd, bounded::Limits::compile())? {
                Ok(output) => output,
                Err((over, _)) => bail!("{name} {case}: {over}"),
            };
            let secs = t0.elapsed().as_secs_f64();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !output.status.success() {
                bail!("{name} {case}: the child failed ({}):\n{stderr}", output.status);
            }
            let (rss_mib, footprint_mib) = peak_mib(&stderr)
                .with_context(|| format!("{name} {case}: no peak in /usr/bin/time's report:\n{stderr}"))?;
            let errors = String::from_utf8_lossy(&output.stdout)
                .lines()
                .find_map(|l| l.strip_prefix("errors ")?.trim().parse().ok())
                .unwrap_or(0);
            peaks.push(Peak { rss_mib, footprint_mib, secs, errors });
        }
        let peaks: [Peak; 3] = peaks.try_into().map_err(|_| anyhow::anyhow!("three cases"))?;
        measured.push((name.clone(), peaks));
    }

    let mut budgets = read_budgets()?;
    let budget_keystrokes = budgets.get("keystrokes").and_then(Value::as_u64);
    let comparable = budget_keystrokes.is_none_or(|k| k == keystrokes as u64);
    let budget_metric = budgets.get("metric").and_then(Value::as_str).unwrap_or("rss").to_owned();
    let same_metric = budget_metric == METRIC;
    let mut over = Vec::new();
    let mut within = 0usize;
    let mut unbudgeted = 0usize;
    let mut tightest: Option<(f64, String)> = None;
    for (name, peaks) in &measured {
        for (case, peak) in CASES.iter().zip(peaks) {
            if !same_metric || (*case == "editor-edits" && !comparable) {
                continue;
            }
            match budgets["budgets"][name][case].as_u64() {
                None => unbudgeted += 1,
                Some(budget) if peak.graded() > budget => over.push(format!("{name} {case}: {} MiB, over its {budget}", peak.graded())),
                Some(budget) => {
                    within += 1;
                    let room = 1.0 - peak.graded() as f64 / budget as f64;
                    if tightest.as_ref().is_none_or(|(r, _)| room < *r) {
                        tightest = Some((room, format!("{name} {case}")));
                    }
                }
            }
        }
    }

    if json_out {
        let rows: Vec<Value> = measured
            .iter()
            .flat_map(|(name, peaks)| {
                CASES.iter().zip(peaks).map(move |(case, p)| {
                    json!({ "program": name, "case": case, "rss_mib": p.rss_mib, "footprint_mib": p.footprint_mib, "secs": p.secs, "errors": p.errors })
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "keystrokes": keystrokes, "metric": METRIC, "peaks": rows }))?);
    } else {
        print_table(&measured, keystrokes);
    }
    if !same_metric {
        println!("the budgets hold the {budget_metric} and this run reads the {METRIC}: not graded");
    }
    if !comparable {
        println!(
            "the budgets were set at {} keystrokes and this run made {keystrokes}: `editor-edits` not graded",
            budget_keystrokes.unwrap_or(0)
        );
    }
    for (name, peaks) in &measured {
        if let Some(p) = peaks.iter().find(|p| p.errors > 0) {
            println!("note: {name} reports {} errors — not the program the reference compiles", p.errors);
        }
    }
    for line in &over {
        println!("OVER BUDGET  {line}");
    }

    if bless {
        let mut table = budgets["budgets"].as_object_mut().map(std::mem::take).unwrap_or_default();
        for (name, peaks) in &measured {
            let entry = table.entry(name.clone()).or_insert_with(|| json!({}));
            for (case, peak) in CASES.iter().zip(peaks) {
                entry[*case] = json!(budget_for(peak.graded()));
            }
        }
        budgets = json!({
            "about": "cargo xtask mem's budgets: MiB of peak memory per program and use case, each its blessed peak plus a quarter (xtask/src/mem.rs)",
            "metric": METRIC,
            "keystrokes": keystrokes,
            "budgets": table,
        });
        std::fs::write(budgets_path(), serde_json::to_string_pretty(&budgets)? + "\n")?;
        println!("blessed: {}", budgets_path().display());
        return Ok(());
    }

    // The line `gates.sh` notes and `status-md` renders: the largest
    // program, every case, and the verdict.
    if let Some((name, peaks)) = measured.last() {
        let short = name.rsplit('/').next().unwrap_or(name);
        let verdict = if over.is_empty() {
            let tight = tightest.map(|(room, at)| format!(", tightest {at} at {:.0}% headroom", room * 100.0)).unwrap_or_default();
            format!("{within} within budget{tight}")
        } else {
            format!("{} OVER BUDGET", over.len())
        };
        let unbudgeted = if unbudgeted > 0 { format!(", {unbudgeted} without one") } else { String::new() };
        println!(
            "mem: {short} check {} MiB, editor open {} MiB, after {keystrokes} edits {} MiB ({METRIC}) — {verdict}{unbudgeted}",
            peaks[0].graded(), peaks[1].graded(), peaks[2].graded()
        );
    }
    if !over.is_empty() {
        bail!("{} peaks over budget (memory-budgets.json); a deliberate rise is `cargo xtask mem --bless`, its diff read first", over.len());
    }
    Ok(())
}

/// A peak's budget: a quarter over it, rounded up to 10 MiB.
fn budget_for(peak_mib: u64) -> u64 {
    ((peak_mib as f64 * HEADROOM / 10.0).ceil() as u64) * 10
}

fn read_budgets() -> Result<Value> {
    match std::fs::read_to_string(budgets_path()) {
        Ok(text) => serde_json::from_str(&text).context("memory-budgets.json"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({ "budgets": {} })),
        Err(e) => Err(e.into()),
    }
}

fn print_table(measured: &[(String, [Peak; 3])], keystrokes: usize) {
    let what = if METRIC == "footprint" { "peak footprint" } else { "peak resident set" };
    println!("{what}, MiB (/usr/bin/time), one process each; the editor runs each of {keystrokes} edits to the fixpoint");
    println!("{:<34} {:>7} {:>12} {:>14} {:>10}", "program", "check", "editor open", format!("after {keystrokes}"), "kept");
    for (name, [check, open, edits]) in measured {
        let kept = edits.graded() as i64 - open.graded() as i64;
        println!("{name:<34} {:>7} {:>12} {:>14} {:>+10}", check.graded(), open.graded(), edits.graded(), kept);
    }
    if METRIC == "footprint" {
        println!("\npeak resident set, MiB (counts pages already purged, which the kernel takes back lazily)");
        for (name, [check, open, edits]) in measured {
            println!("{name:<34} {:>7} {:>12} {:>14}", check.rss_mib, open.rss_mib, edits.rss_mib);
        }
    }
}

/// The peak resident set, and on macOS the peak footprint, from
/// `/usr/bin/time -l` (bytes) or GNU `time -v` (KiB).
fn peak_mib(report: &str) -> Option<(u64, Option<u64>)> {
    let mut rss = None;
    let mut footprint = None;
    for line in report.lines().map(str::trim) {
        let number = |n: &str| n.trim().parse::<u64>().ok();
        if let Some(n) = line.strip_suffix("maximum resident set size") {
            rss = number(n).map(|b| b >> 20);
        } else if let Some(n) = line.strip_suffix("peak memory footprint") {
            footprint = number(n).map(|b| b >> 20);
        } else if let Some(n) = line.strip_prefix("Maximum resident set size (kbytes):") {
            rss = number(n).map(|k| k >> 10);
        }
    }
    Some((rss?, footprint))
}

/// One use case, in this process, for the parent to measure: `file` under
/// the settings `options` give, as [`run`] describes each case. Prints the
/// error count the parent reports.
pub(crate) fn child(kanagawa_root: &Path, file: &Path, case: &str, keystrokes: usize, options: &[String]) -> Result<()> {
    let settings = crate::conformance::settings_for(kanagawa_root, options);
    let path = camino::Utf8PathBuf::from_path_buf(file.to_path_buf()).map_err(|p| anyhow::anyhow!("{} is not UTF-8", p.display()))?;
    let text = std::fs::read_to_string(&path).with_context(|| path.to_string())?;
    let mut db = kg_db::Db::new(settings);
    let diagnostics = match case {
        "check" => {
            let root = db.set_file_text(&path, text);
            db.check(&[root])
        }
        "editor-open" | "editor-edits" => {
            // As `kg lsp` starts.
            kg_alloc::purge_eagerly();
            db.set_pass_budget(kg_db::PassBudget::EDITOR);
            let root = db.set_file_text(&path, text.clone());
            let mut last = db.diagnostics_layered(&[root], root, drop);
            let edits = if case == "editor-edits" { keystrokes } else { 0 };
            // A trailing space alternated, as the bench's `keystroke` stage
            // types: the text changes every time, and no declaration does.
            for nth in 0..edits {
                let root = db.set_file_text(&path, format!("{text}{}", " ".repeat(nth % 2 + 1)));
                last = db.diagnostics_layered(&[root], root, drop);
            }
            last
        }
        other => bail!("no use case {other}"),
    };
    println!("errors {}", diagnostics.iter().filter(|d| d.is_error()).count());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both reports the parent reads: macOS's `-l` in bytes, GNU's `-v` in
    /// KiB — the same 700 MiB from each.
    #[test]
    fn a_peak_reads_from_either_time() {
        let macos = "        2.79 real         2.50 user         0.25 sys\n\
                     \x20          734003200  maximum resident set size\n\
                     \x20                  0  average shared memory size\n\
                     \x20          629145600  peak memory footprint\n";
        assert_eq!(peak_mib(macos), Some((700, Some(600))));
        let gnu = "\tCommand being timed: \"xtask\"\n\tMaximum resident set size (kbytes): 716800\n\tExit status: 0\n";
        assert_eq!(peak_mib(gnu), Some((700, None)));
        assert_eq!(peak_mib("no report"), None);
    }

    #[test]
    fn a_budget_is_a_quarter_over_its_peak_rounded_up() {
        assert_eq!(budget_for(700), 880);
        assert_eq!(budget_for(41), 60);
        assert_eq!(budget_for(1300), 1630);
    }
}
