//! `cargo xtask ptdiff` — our walk against the reference's, call by call.
//!
//! For every corpus program `cargo xtask ptrace` recorded a walk for, this
//! builds the same program here, walks it through `kg-ptbridge`'s trace
//! backend, and compares the two call sequences over the region that is the
//! front end's alone: depth 0, before `Codegen` (`abi.md` §5.4, §14.2).
//! Three readings per program, from strict to loose:
//!
//! * **exact** — every call, argument and result the same, locations
//!   included (the `_valid` flag excepted: the reference leaves it
//!   indeterminate, §6.1);
//! * **structural** — the same with every `SetLocation2`/`UnknownLocation`
//!   line dropped and every location token blanked, because the reference's
//!   spans differ from ours by construction on some nodes (divergence #25)
//!   and a position is never what the backend decides on;
//! * otherwise the **first divergent call**, ours against theirs, or the
//!   node kind our walk **declined** if the two agree up to where it stopped.
//!
//! A program our frontend rejects while the reference walked it is its own
//! row — the trap `known-divergences.md` records, counted rather than
//! folded into a divergence. The histograms at the end — divergent pairs
//! by `(theirs, ours)`, declines by reason — are the work list: the most
//! frequent first divergence is the rewrite worth porting next.
//!
//! A gate is a driver over threads (`docs/gates.md`): programs go to workers
//! that each own a `Db`, results land in slots by program.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use kg_pttrace::Line;

use crate::conformance::settings_for;
use crate::ptrace::{Loaded, Program, programs};

#[derive(Debug, Clone)]
enum Verdict {
    Exact,
    /// Structurally equal; the first exact mismatch, for the record.
    Structural { first: usize, ours: String, theirs: String },
    Diverged { at: usize, ours: String, theirs: String },
    /// Agreed up to our end, where the walk declined a node.
    Declined { reason: String, at: usize, of: usize },
    /// Our frontend reports an error; the reference walked.
    Rejected(String),
    /// The walk ended before the reference's without declining — fewer
    /// calls, all agreeing.
    Short { ours: usize, theirs: usize },
    Failed(String),
}

/// An owned sink for the trace writer that the caller can read back.
#[derive(Clone, Default, Debug)]
struct SharedBuf(std::sync::Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("no panics hold this lock").extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The comparison region of a recorded trace: depth-0 calls before `Codegen`.
fn reference_calls(text: &str) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    for line in kg_pttrace::parse(text).map_err(|e| anyhow::anyhow!(e))? {
        match &line {
            Line::Call { depth, name, .. } => {
                if name == "Codegen" {
                    break;
                }
                if *depth == 0 {
                    out.push(line);
                }
            }
            Line::Meta { .. } => {}
        }
    }
    Ok(out)
}

fn our_calls(text: &str) -> Result<Vec<Line>> {
    Ok(kg_pttrace::parse(text)
        .map_err(|e| anyhow::anyhow!(e))?
        .into_iter()
        .filter(|l| matches!(l, Line::Call { name, .. } if name != "Codegen"))
        .collect())
}

/// A location token without its `_valid` digit.
fn strip_valid(tok: &str) -> String {
    if tok.starts_with('@')
        && let Some(i) = tok.rfind(':')
    {
        return tok[..i].to_owned();
    }
    tok.to_owned()
}

/// The exact reading: name, arguments, result.
fn exact(line: &Line) -> String {
    match line {
        Line::Call { name, args, ret, .. } => {
            let mut s = name.clone();
            for a in args {
                s.push(' ');
                s.push_str(&strip_valid(a));
            }
            s.push_str(" -> ");
            s.push_str(ret);
            s
        }
        Line::Meta { .. } => String::new(),
    }
}

/// The structural reading: no location lines, no location tokens.
fn structural(line: &Line) -> Option<String> {
    match line {
        Line::Call { name, args, ret, .. } => {
            if name == "SetLocation2" || name == "UnknownLocation" {
                return None;
            }
            let mut s = name.clone();
            for a in args {
                s.push(' ');
                s.push_str(if a.starts_with('@') { "@" } else { a });
            }
            s.push_str(" -> ");
            s.push_str(ret);
            Some(s)
        }
        Line::Meta { .. } => None,
    }
}

fn compare(ours: &[Line], theirs: &[Line], declined: Option<&str>) -> Verdict {
    let o: Vec<(usize, String)> =
        ours.iter().enumerate().filter_map(|(i, l)| structural(l).map(|s| (i, s))).collect();
    let t: Vec<(usize, String)> =
        theirs.iter().enumerate().filter_map(|(i, l)| structural(l).map(|s| (i, s))).collect();
    let common = o.len().min(t.len());
    for k in 0..common {
        if o[k].1 != t[k].1 {
            return Verdict::Diverged { at: k, ours: exact(&ours[o[k].0]), theirs: exact(&theirs[t[k].0]) };
        }
    }
    if let Some(reason) = declined {
        return Verdict::Declined { reason: reason.to_owned(), at: o.len(), of: t.len() };
    }
    if o.len() < t.len() {
        return Verdict::Short { ours: o.len(), theirs: t.len() };
    }
    if o.len() > t.len() {
        return Verdict::Diverged { at: common, ours: exact(&ours[o[common].0]), theirs: "<end>".to_owned() };
    }
    // Structurally equal; now exactly?
    let oe: Vec<String> = ours.iter().map(exact).collect();
    let te: Vec<String> = theirs.iter().map(exact).collect();
    for k in 0..oe.len().min(te.len()) {
        if oe[k] != te[k] {
            return Verdict::Structural { first: k, ours: oe[k].clone(), theirs: te[k].clone() };
        }
    }
    if oe.len() != te.len() {
        let k = oe.len().min(te.len());
        return Verdict::Structural {
            first: k,
            ours: oe.get(k).cloned().unwrap_or_else(|| "<end>".into()),
            theirs: te.get(k).cloned().unwrap_or_else(|| "<end>".into()),
        };
    }
    Verdict::Exact
}

/// Walk one program here and compare.
fn judge(db: &mut kg_db::Db, p: &Program, theirs_text: &str) -> Verdict {
    let theirs = match reference_calls(theirs_text) {
        Ok(t) => t,
        Err(e) => return Verdict::Failed(format!("reading the reference's trace: {e}")),
    };
    let path = camino::Utf8PathBuf::from_path_buf(p.cwd.join(&p.file_arg)).expect("UTF-8 paths");
    let root = match &p.source {
        Some(source) => db.set_file_text(&path, source.clone()),
        None => {
            if db.vfs().read_path(&path).is_none() {
                return Verdict::Failed(format!("cannot read {path}"));
            }
            db.file_id(&path)
        }
    };
    let diagnostics = db.check(&[root]);
    if let Some(e) = diagnostics.iter().find(|d| d.is_error()) {
        return Verdict::Rejected(e.message.chars().take(100).collect());
    }
    let buf = SharedBuf::default();
    let declined = {
        let mut trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
        let mut options = kg_backend_sys::options::OptionValues::harness_defaults();
        options.target_device = db.settings().target_device.clone();
        let outcome = kg_ptbridge::program::run(db, &[root], options, None, &mut trace);
        trace.flush();
        outcome.declined.map(|d| d.reason)
    };
    let text = String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned();
    let ours = match our_calls(&text) {
        Ok(o) => o,
        Err(e) => return Verdict::Failed(format!("reading our trace: {e}")),
    };
    compare(&ours, &theirs, declined.as_deref())
}

/// Write both traces of one program under `target/ptrace/diff/<name>/`, for
/// reading side by side.
pub fn dump(kanagawa_root: &Path, name: &str) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let Some(p) = programs(kanagawa_root)?.into_iter().find(|p| p.name == name) else {
        anyhow::bail!("no program named {name}");
    };
    let theirs = match crate::ptrace::load(kanagawa_root, &p)? {
        Loaded::Fresh(_, theirs) => theirs,
        Loaded::Stale => anyhow::bail!("{name}'s recording is stale — run `cargo xtask ptrace --filter {name}`"),
        Loaded::Unrecorded => anyhow::bail!("{name} is not recorded — run `cargo xtask ptrace --filter {name}`"),
    };
    let mut db = kg_db::Db::new(settings_for(kanagawa_root, &p.options));
    let path = camino::Utf8PathBuf::from_path_buf(p.cwd.join(&p.file_arg)).expect("UTF-8 paths");
    let root = match &p.source {
        Some(source) => db.set_file_text(&path, source.clone()),
        None => db.file_id(&path),
    };
    let buf = SharedBuf::default();
    let outcome = {
        let mut trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
        let mut options = kg_backend_sys::options::OptionValues::harness_defaults();
        options.target_device = db.settings().target_device.clone();
        let outcome = kg_ptbridge::program::run(&db, &[root], options, None, &mut trace);
        trace.flush();
        outcome
    };
    let program = db.program(&[root]);
    for &m in &program.files {
        let n = program.graph.redundant_imports(m).count();
        if n > 0 {
            let path = m.file().and_then(|f| db.path(f)).map(|p| p.to_string()).unwrap_or_default();
            let names: Vec<String> = program
                .graph
                .redundant_imports(m)
                .map(|i| format!("{:?}", i.name))
                .collect();
            println!("redundant imports in {path}: {names:?}");
        }
    }
    let report = &db.looped(&[root]).out.report;
    println!(
        "loop: rounds {} made {} invalid {} left {} budget exhausted {}",
        report.rounds, report.made, report.invalid, report.left, report.budget_exhausted
    );
    let dir = crate::ptrace::ptrace_dir().join("diff").join(name.replace("::", "/"));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("theirs.trace"), theirs)?;
    std::fs::write(dir.join("ours.trace"), &*buf.0.lock().expect("no panics hold this lock"))?;
    println!("wrote {}/{{ours,theirs}}.trace", dir.display());
    if let Some(d) = &outcome.declined {
        println!("declined: {} at {:?}", d.reason, d.at.head);
    }
    let ours_text = String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned();
    let theirs_text = std::fs::read_to_string(dir.join("theirs.trace"))?;
    let (ours, theirs) = (our_calls(&ours_text)?, reference_calls(&theirs_text)?);
    match compare(&ours, &theirs, outcome.declined.as_ref().map(|d| d.reason.as_str())) {
        Verdict::Diverged { at, ours: o, theirs: t } => {
            // The structural index, mapped back to the sequence numbers.
            let seq_of = |lines: &[Line], k: usize| {
                lines
                    .iter()
                    .filter(|l| structural(l).is_some())
                    .nth(k)
                    .and_then(|l| match l {
                        Line::Call { seq, .. } => Some(*seq),
                        Line::Meta { .. } => None,
                    })
            };
            println!(
                "diverged at structural #{at}: ours seq {:?} `{o}` / theirs seq {:?} `{t}`",
                seq_of(&ours, at),
                seq_of(&theirs, at)
            );
        }
        other => println!("{other:?}"),
    }
    Ok(())
}

pub fn run(kanagawa_root: &Path, filter: Option<&str>, verbose: bool, max_failures: Option<usize>, list: bool) -> Result<()> {
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let mut work: Vec<(Program, String)> = Vec::new();
    let mut unrecorded = 0usize;
    let mut stale = Vec::new();
    for p in programs(kanagawa_root)? {
        if !filter.is_none_or(|f| p.name.contains(f)) {
            continue;
        }
        match crate::ptrace::load(kanagawa_root, &p)? {
            Loaded::Fresh(run, text) if run.lines > 0 => work.push((p, text)),
            Loaded::Fresh(..) => {}
            Loaded::Stale => stale.push(p.name),
            Loaded::Unrecorded => unrecorded += 1,
        }
    }
    if work.is_empty() {
        anyhow::bail!(
            "nothing to compare ({unrecorded} programs unrecorded, {} stale) — run `cargo xtask ptrace` first",
            stale.len()
        );
    }
    let started = std::time::Instant::now();

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Verdict>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(settings_for(kanagawa_root, &[]));
                let mut current: Option<Vec<String>> = None;
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= work.len() {
                        break;
                    }
                    let (p, text) = &work[i];
                    if current.as_deref() != Some(p.options.as_slice()) {
                        db.set_settings(settings_for(kanagawa_root, &p.options));
                        current = Some(p.options.clone());
                    }
                    let verdict = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| judge(&mut db, p, text))) {
                        Ok(v) => v,
                        Err(e) => Verdict::Failed(format!(
                            "panic: {}",
                            e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default()
                        )),
                    };
                    *slots[i].lock().expect("no panics hold this lock") = Some(verdict);
                }
            });
        }
    });

    let mut exact = 0usize;
    let mut structural = 0usize;
    let mut diverged = 0usize;
    let mut declined = 0usize;
    let mut rejected = 0usize;
    let mut short = 0usize;
    let mut failed = 0usize;
    let mut pairs: BTreeMap<(String, String), (usize, String)> = BTreeMap::new();
    let mut reasons: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut structural_first: BTreeMap<(String, String), (usize, String)> = BTreeMap::new();
    let mut lines = Vec::new();
    // `structural` holds the exact programs too: it is the set the headline
    // counts, and an exact program turning structural leaves only `exact`.
    let mut ledger = crate::ratchet::Ledger::new("ptdiff");
    for set in ["exact", "structural", "rejected", "failed"] {
        ledger.set(set);
    }
    for ((p, _), slot) in work.iter().zip(slots) {
        let v = slot.into_inner().expect("threads joined").expect("every slot filled");
        match &v {
            Verdict::Exact => {
                ledger.add("exact", &p.name);
                ledger.add("structural", &p.name);
            }
            Verdict::Structural { .. } => ledger.add("structural", &p.name),
            Verdict::Rejected(_) => ledger.add("rejected", &p.name),
            Verdict::Failed(_) => ledger.add("failed", &p.name),
            Verdict::Declined { reason, .. } => ledger.histogram("declined", reason),
            Verdict::Diverged { ours, theirs, .. } => {
                ledger.histogram("first divergence", &format!("{} / {}", head(theirs), head(ours)));
            }
            Verdict::Short { .. } => {}
        }
        if list {
            let word = match &v {
                Verdict::Exact => "exact",
                Verdict::Structural { .. } => "structural",
                Verdict::Diverged { .. } => "diverged",
                Verdict::Declined { .. } => "declined",
                Verdict::Rejected(_) => "rejected",
                Verdict::Short { .. } => "short",
                Verdict::Failed(_) => "failed",
            };
            match &v {
                Verdict::Diverged { at, ours, theirs } => {
                    println!("  {word} {} #{at}: ours `{ours}` theirs `{theirs}`", p.name);
                }
                _ => println!("  {word} {}", p.name),
            }
        }
        match &v {
            Verdict::Exact => exact += 1,
            Verdict::Structural { first, ours, theirs } => {
                structural += 1;
                let key = (head(theirs), head(ours));
                let e = structural_first.entry(key).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!("{} #{first}: ours `{ours}` theirs `{theirs}`", p.name);
                }
            }
            Verdict::Diverged { at, ours, theirs } => {
                diverged += 1;
                let key = (head(theirs), head(ours));
                let e = pairs.entry(key).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!("{} #{at}: ours `{ours}` theirs `{theirs}`", p.name);
                }
                if verbose {
                    lines.push(format!("  DIVERGED {} #{at}\n    ours:   {ours}\n    theirs: {theirs}", p.name));
                }
            }
            Verdict::Declined { reason, at, of } => {
                declined += 1;
                let e = reasons.entry(reason.clone()).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!("{} at {at}/{of}", p.name);
                }
            }
            Verdict::Rejected(msg) => {
                rejected += 1;
                if verbose {
                    lines.push(format!("  REJECTED {}: {msg}", p.name));
                }
            }
            Verdict::Short { ours, theirs } => {
                short += 1;
                if verbose {
                    lines.push(format!("  SHORT {}: {ours} of {theirs}", p.name));
                }
            }
            Verdict::Failed(msg) => {
                failed += 1;
                lines.push(format!("  FAILED {}: {msg}", p.name));
            }
        }
    }
    println!(
        "ptdiff: {} programs compared ({unrecorded} unrecorded, {} stale) in {:.1}s",
        work.len(),
        stale.len(),
        started.elapsed().as_secs_f64()
    );
    println!("  exact {exact}, structural {structural}, diverged {diverged}, declined {declined}, rejected {rejected}, short {short}, failed {failed}");
    if !pairs.is_empty() {
        println!("  first divergence, by (theirs, ours):");
        let mut v: Vec<_> = pairs.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for ((t, o), (n, example)) in v.iter().take(20) {
            println!("    {n:5}  {t} / {o}   e.g. {example}");
        }
    }
    if !reasons.is_empty() {
        println!("  declined, by reason:");
        let mut v: Vec<_> = reasons.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (r, (n, example)) in v.iter().take(20) {
            println!("    {n:5}  {r}   e.g. {example}");
        }
    }
    if !structural_first.is_empty() {
        println!("  structural only — first exact mismatch, by (theirs, ours):");
        let mut v: Vec<_> = structural_first.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for ((t, o), (n, example)) in v.iter().take(10) {
            println!("    {n:5}  {t} / {o}   e.g. {example}");
        }
    }
    for l in &lines {
        println!("{l}");
    }
    for name in stale.iter().take(10) {
        println!("  STALE {name}: recorded from other inputs — not graded");
    }
    if filter.is_none() {
        for (name, n) in [
            ("graded", work.len()),
            ("unrecorded", unrecorded),
            ("stale", stale.len()),
            ("exact", exact),
            ("structural-only", structural),
            ("diverged", diverged),
            ("declined", declined),
            ("rejected", rejected),
            ("short", short),
            ("failed", failed),
        ] {
            ledger.total(name, n);
        }
        crate::ratchet::enforce(ledger)?;
    }
    if !stale.is_empty() {
        anyhow::bail!("{} recordings are stale — `cargo xtask ptrace` re-records them", stale.len());
    }
    let not_exact = work.len() - exact;
    if let Some(max) = max_failures
        && not_exact > max
    {
        anyhow::bail!("{not_exact} programs are not exact, above the ratchet of {max}");
    }
    if failed > 0 {
        anyhow::bail!("{failed} programs could not be compared");
    }
    Ok(())
}

/// The function name of a rendered call.
fn head(line: &str) -> String {
    line.split(' ').next().unwrap_or("").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trace lines, parsed as the gate parses a recording.
    fn calls(text: &str) -> Vec<Line> {
        kg_pttrace::parse(text).expect("a well-formed trace")
    }

    fn one(text: &str) -> Line {
        calls(text).pop().expect("one line")
    }

    /// The structural reading keeps only calls that are not location calls:
    /// `SetLocation2` and `UnknownLocation` say where, never what, and the
    /// reference's spans differ from ours by construction on some nodes
    /// (divergence #25), so a location call may be present on one side
    /// only. `#` lines are not calls. Every structural count — the headline
    /// `structural` set among them — rests on which lines this drops.
    #[test]
    fn structural_drops_location_calls_and_meta_lines() {
        assert_eq!(structural(&one("00000001 0 SetLocation2 @0:1:1-1:16:0 -> -")), None);
        assert_eq!(structural(&one("00000007 0 UnknownLocation -> -")), None);
        assert_eq!(structural(&one("#epoch 1")), None);
        assert_eq!(structural(&one("00000006 0 ParseBaseList _ -> n3")).as_deref(), Some("ParseBaseList _ -> n3"));
    }

    /// A location token (`@<file>:<span>:<valid>`, `kg_pttrace::Location::render`)
    /// is blanked to `@` wherever it appears among the arguments; every
    /// other token, and the result, is compared as written. A string whose
    /// text starts with `@` — `"@.cmdargs"`, a real `ParseIdentifier`
    /// argument — renders quoted and is not a location.
    #[test]
    fn structural_blanks_location_tokens_and_keeps_the_rest() {
        let line = one(r#"00000005 0 ParseX n1 @0:1:2-3:4:1 "@.cmdargs" 7 -> n2"#);
        assert_eq!(structural(&line).as_deref(), Some(r#"ParseX n1 @ "@.cmdargs" 7 -> n2"#));
    }

    /// The same calls, locations included, are exact — and the `_valid`
    /// flag, the last field of a location, is not compared: the reference
    /// leaves it indeterminate (`abi.md` §6.1).
    #[test]
    fn identical_walks_are_exact_whatever_the_valid_flags() {
        let ours = calls("00000001 0 SetLocation2 @0:1:1-1:16:0 -> -\n00000002 0 ParseIdentifier \"a\" -> n1\n");
        let theirs = calls("00000001 0 SetLocation2 @0:1:1-1:16:1 -> -\n00000002 0 ParseIdentifier \"a\" -> n1\n");
        let v = compare(&ours, &theirs, None);
        assert!(matches!(v, Verdict::Exact), "{v:?}");
    }

    /// Walks that differ only in a span are structural, and the record is
    /// the first exact mismatch by its index among *all* calls, location
    /// calls counted, rendered with the `_valid` flag stripped.
    #[test]
    fn a_span_difference_is_structural_at_its_exact_index() {
        let ours = calls("00000001 0 ParseA -> n1\n00000002 0 SetLocation2 @0:1:1-1:16:0 -> -\n00000003 0 ParseB n1 -> n2\n");
        let theirs = calls("00000001 0 ParseA -> n1\n00000002 0 SetLocation2 @0:1:1-1:17:1 -> -\n00000003 0 ParseB n1 -> n2\n");
        let Verdict::Structural { first, ours, theirs } = compare(&ours, &theirs, None) else { panic!("not structural") };
        assert_eq!((first, ours.as_str(), theirs.as_str()), (1, "SetLocation2 @0:1:1-1:16 -> -", "SetLocation2 @0:1:1-1:17 -> -"));
    }

    /// A location call on one side only leaves the walks structural, not
    /// diverged — in the middle of the walk and at its end, where the
    /// shorter side reads `<end>`.
    #[test]
    fn a_location_call_on_one_side_only_is_structural() {
        let theirs = calls("00000001 0 ParseA -> n1\n00000002 0 ParseB n1 -> n2\n");
        let middle = calls("00000001 0 ParseA -> n1\n00000002 0 UnknownLocation -> -\n00000003 0 ParseB n1 -> n2\n");
        let Verdict::Structural { first, ours, theirs: t } = compare(&middle, &theirs, None) else { panic!("not structural") };
        assert_eq!((first, ours.as_str(), t.as_str()), (1, "UnknownLocation -> -", "ParseB n1 -> n2"));
        let end = calls("00000001 0 ParseA -> n1\n00000002 0 ParseB n1 -> n2\n00000003 0 SetLocation2 @0:1:1-1:2:0 -> -\n");
        let Verdict::Structural { first, ours, theirs: t } = compare(&end, &theirs, None) else { panic!("not structural") };
        assert_eq!((first, ours.as_str(), t.as_str()), (2, "SetLocation2 @0:1:1-1:2 -> -", "<end>"));
    }

    /// The first structural mismatch is the divergence: `at` counts
    /// structural calls only (location calls skipped on each side
    /// independently), while `ours`/`theirs` are the exact renderings of
    /// the two calls, locations and all. Their heads key the gate's
    /// `first divergence` histogram, the work list.
    #[test]
    fn the_first_structural_mismatch_diverges_with_exact_renderings() {
        let ours = calls(concat!(
            "00000001 0 SetLocation2 @0:1:1-1:2:0 -> -\n",
            "00000002 0 ParseA n1 -> n2\n",
            "00000003 0 ParseB @0:2:1-2:4:1 -> n3\n",
        ));
        let theirs = calls(concat!(
            "00000001 0 SetLocation2 @0:5:5-5:6:1 -> -\n",
            "00000002 0 UnknownLocation -> -\n",
            "00000003 0 ParseA n1 -> n2\n",
            "00000004 0 ParseC @0:2:1-2:5:0 -> n3\n",
        ));
        let Verdict::Diverged { at, ours, theirs } = compare(&ours, &theirs, None) else { panic!("not diverged") };
        assert_eq!((at, ours.as_str(), theirs.as_str()), (1, "ParseB @0:2:1-2:4 -> n3", "ParseC @0:2:1-2:5 -> n3"));
    }

    /// A result is part of the call: the same call answered with another
    /// node diverges.
    #[test]
    fn a_different_result_diverges() {
        let v = compare(&calls("00000001 0 ParseA -> n1\n"), &calls("00000001 0 ParseA -> n2\n"), None);
        assert!(matches!(v, Verdict::Diverged { at: 0, .. }), "{v:?}");
    }

    /// A walk that agreed up to where it declined is `Declined`, `at` and
    /// `of` counting structural calls — ours and the reference's.
    #[test]
    fn a_decline_after_agreement_counts_structural_calls() {
        let ours = calls("00000001 0 SetLocation2 @0:1:1-1:2:0 -> -\n00000002 0 ParseA -> n1\n");
        let theirs = calls(concat!(
            "00000001 0 SetLocation2 @0:1:1-1:2:0 -> -\n",
            "00000002 0 ParseA -> n1\n",
            "00000003 0 SetLocation2 @0:1:3-1:4:0 -> -\n",
            "00000004 0 ParseB n1 -> n2\n",
        ));
        let Verdict::Declined { reason, at, of } = compare(&ours, &theirs, Some("lambda")) else { panic!("not declined") };
        assert_eq!((reason.as_str(), at, of), ("lambda", 1, 2));
    }

    /// A declined walk is never exact, even when every call it made agrees
    /// and it made as many as the reference: a walk that did not finish is
    /// not one the backend was told the whole of.
    #[test]
    fn a_declined_walk_is_never_exact() {
        let walk = calls("00000001 0 ParseA -> n1\n");
        let v = compare(&walk, &walk, Some("lambda"));
        assert!(matches!(v, Verdict::Declined { at: 1, of: 1, .. }), "{v:?}");
    }

    /// A divergence before the point of decline is a divergence: the
    /// decline only explains a walk that agreed up to its end.
    #[test]
    fn a_divergence_before_the_decline_wins() {
        let ours = calls("00000001 0 ParseA -> n1\n");
        let theirs = calls("00000001 0 ParseB -> n1\n00000002 0 ParseC -> n2\n");
        let v = compare(&ours, &theirs, Some("lambda"));
        assert!(matches!(v, Verdict::Diverged { at: 0, .. }), "{v:?}");
    }

    /// A walk that stopped early, agreeing and without declining, is
    /// `Short`, counted in structural calls.
    #[test]
    fn an_early_end_without_a_decline_is_short() {
        let ours = calls("00000001 0 SetLocation2 @0:1:1-1:2:0 -> -\n00000002 0 ParseA -> n1\n");
        let theirs = calls("00000001 0 ParseA -> n1\n00000002 0 ParseB n1 -> n2\n");
        let Verdict::Short { ours, theirs } = compare(&ours, &theirs, None) else { panic!("not short") };
        assert_eq!((ours, theirs), (1, 2));
    }

    /// A walk that goes on past the reference's end diverges there, against
    /// `<end>`.
    #[test]
    fn calls_past_the_references_end_diverge_against_end() {
        let ours = calls("00000001 0 ParseA -> n1\n00000002 0 ParseZ @0:1:1-1:2:1 -> n9\n");
        let theirs = calls("00000001 0 ParseA -> n1\n");
        let Verdict::Diverged { at, ours, theirs } = compare(&ours, &theirs, None) else { panic!("not diverged") };
        assert_eq!((at, ours.as_str(), theirs.as_str()), (1, "ParseZ @0:1:1-1:2 -> n9", "<end>"));
    }
}
