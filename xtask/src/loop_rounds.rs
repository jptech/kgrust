//! `cargo xtask loop-rounds`: what the loop carried across an edit could
//! keep (sprint W7.5d step 5d-0, `docs/plans/loop-across-edits.md` §5) — a
//! measurement, not a gate.
//!
//! In round *r* the loop's work on a library module *M* is a function of
//! *M*'s state after round *r − 1*, the copies the census decided into *M*
//! in round *r*, and what *M*'s passes read across the module boundary
//! (§3's (1), (2) and (3)). D1 replays a kept round while all three answer
//! as before. This measures (2): over `edit-identity`'s edits
//! (`crate::edits`, the same roots), the loop runs to its fixpoint before
//! the edit and after it, and for each library module — every module but
//! the edited one — the first round whose copies into it differ. The rounds
//! before it are what D1 could keep. (3) is not measured, so the figure is
//! an upper bound on D1's.
//!
//! A copy is spelled from the census as `instantiate` reads it: template,
//! argument key, `validateArg`'s stamps, and the occurrence it is made
//! from — its module, position and expansion, and the argument subtree with
//! every node's position, provenance, kind, expansion and type. An
//! expansion is spelled as its frames — instance (content-addressed),
//! site, round — and a file as its path, so nothing a run interns is
//! compared. Three readings of "differs":
//!
//! * **as written** — positions as they are: what D1, keyed on what was
//!   read, compares, since a copy's nodes carry their sites' ranges;
//! * **relocated** — positions in the edited file carried through the edit
//!   first: what D1 could keep if it moved a kept round's ranges with the
//!   text;
//! * **relocated, with mentions** — and the occurrences the census met in
//!   *M* (what `resolve` renames there) counted as *M*'s input too;
//! * **relocated, state checked** — and a round kept only while *M*'s
//!   whole state (every arena node, provenance and type, positions
//!   relocated) at each census before it, and at the end, is the old
//!   run's. This sees what (3) did to the state, not what it read: an
//!   upper bound still, but one (3) has been allowed to lower.
//!
//! The state is also checked for every module a reading keeps whole: does
//! it end as it ended before? A module that does not is one whose copies
//! agreed and whose reads across the boundary did not.
//!
//! Time is `kg_sem::profile::record`'s clock in the run after the edit, the
//! run D1 would shorten: each pass's visit to each module, inference's walk
//! of each module included, per round. A module's passes before the loop
//! (1–7) are kept with it unless it is new. The whole-program steps — the
//! symbol maps, inference's own bookkeeping, the census's union — are no
//! module's, and are reported apart.
//!
//! Each root is also measured twice without an edit: every library round
//! must then be kept under every reading, or the spelling holds something a
//! run interns and the numbers mean nothing.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use kg_base::{FileId, FxHashMap};
use kg_db::{Db, ModuleSource};
use kg_sem::snapshot::{position, Final, Speller, CLOSE, OPEN, SEP};

use crate::edits::{Kind, Splice};
use crate::ptrace::{programs, Program};

/// The editor's rounds (`kg_db::PassBudget::INTERACTIVE`): rounds 0–3.
const EDITOR_ROUNDS: usize = 4;

/// How two runs' inputs to a module's round are compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    Written,
    Relocated,
    Mentions,
    /// Relocated, and a round kept only while the module's state at each
    /// census before it, and at the end, is the old run's: what (3) did to
    /// the state, though not what it read.
    State,
}

impl Reading {
    const ALL: [Reading; 4] = [Reading::Written, Reading::Relocated, Reading::Mentions, Reading::State];

    fn name(self) -> &'static str {
        match self {
            Reading::Written => "as written",
            Reading::Relocated => "relocated",
            Reading::Mentions => "relocated, with mentions",
            Reading::State => "relocated, state checked",
        }
    }
}

/// One run of the loop, as 5d-0 reads it.
#[derive(Clone, Debug, Default)]
struct Run {
    /// Its modules in program order, by path.
    modules: Vec<String>,
    /// Per round — one per census, the last deciding nothing — per module:
    /// the copies decided into it, spelled, in the census's order.
    copies: Vec<Vec<Vec<String>>>,
    /// Per round, per module: the occurrences the census met there.
    mentions: Vec<Vec<Vec<String>>>,
    /// Per module: its time before the loop (passes 1–7), then per round.
    time: Vec<Vec<u64>>,
    /// The time no module is charged with.
    whole: u64,
    /// Per module: its state when the loop ends.
    finals: Vec<Final>,
    /// Per round, per module: its state at the round's census.
    states: Vec<Vec<Final>>,
    /// At each census, then at the end: [`size`].
    sizes: Vec<[u64; 3]>,
    end: [u64; 3],
}

/// Kept and in all, `[kept, all]`: module-rounds and nanoseconds, every
/// round and rounds 0–3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Share {
    rounds: [u64; 2],
    nanos: [u64; 2],
    rounds4: [u64; 2],
    nanos4: [u64; 2],
}

impl Share {
    fn add(&mut self, o: &Share) {
        for (a, b) in [(&mut self.rounds, o.rounds), (&mut self.nanos, o.nanos), (&mut self.rounds4, o.rounds4), (&mut self.nanos4, o.nanos4)] {
            a[0] += b[0];
            a[1] += b[1];
        }
    }
}

/// Modules and their nanoseconds, `[all, agreeing]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Check {
    modules: [u64; 2],
    nanos: [u64; 2],
}

/// One edit, measured.
#[derive(Clone, Debug)]
struct Outcome {
    kind: Kind,
    /// Census rounds before and after.
    rounds: (usize, usize),
    library_modules: usize,
    share: [Share; 4],
    /// Of the library modules kept whole as written and relocated: how
    /// many, and how many end in the state they ended in before.
    finals: [Check; 2],
    /// Whether the edited module's own occurrences in round 0 differ: as
    /// written, relocated, and as the multiset of what they ask for.
    requests: [bool; 3],
    /// After the edit: the loop, the edited module's own time, the
    /// library's, the whole-program steps.
    loop_ns: u64,
    edited_ns: u64,
    library_ns: u64,
    whole_ns: u64,
    /// The first library module to part, relocated: where and how.
    first: Option<String>,
    /// The library modules kept whole, relocated, that end otherwise.
    unequal: Vec<String>,
    /// The library modules whose state parts before their copies do.
    earlier: Vec<String>,
    /// After the edit, the program's [`size`] at round 0's census and at
    /// the end: what D1 would keep, and how much of it is rewritten after
    /// the first round.
    sizes: [[u64; 3]; 2],
}

pub fn run(kanagawa_root: &Path, filter: Option<&str>, every: usize, jobs: Option<usize>, verbose: bool) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let started = std::time::Instant::now();
    let mut work: Vec<Program> = programs(kanagawa_root)?
        .into_iter()
        .filter(|p| p.name.starts_with("file::") && filter.is_none_or(|f| p.name.contains(f)))
        .collect();
    if every > 1 {
        work = work.into_iter().step_by(every).collect();
    }
    let size = |p: &Program| std::fs::metadata(p.cwd.join(&p.file_arg)).map_or(0, |m| m.len());
    work.sort_by_key(|p| std::cmp::Reverse(size(p)));
    if work.is_empty() {
        anyhow::bail!("no root matched");
    }
    let library = kanagawa_root.join("library");
    let workers = jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(6));
    let next = AtomicUsize::new(0);
    type Graded = Result<(Vec<Outcome>, bool), String>;
    let slots: Vec<Mutex<Option<Graded>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(p) = work.get(i) else { break };
                let settings = crate::conformance::settings_for(kanagawa_root, &p.options);
                let graded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| grade_root(&settings, p, &library)))
                    .unwrap_or_else(|e| {
                        let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()));
                        Err(anyhow::anyhow!("panic: {}", what.unwrap_or_default()))
                    })
                    .map_err(|e| format!("{e:#}"));
                *slots[i].lock().expect("no panics hold this lock") = Some(graded);
            });
        }
    });
    let mut outcomes: Vec<(String, Outcome)> = Vec::new();
    let (mut steady, mut failed) = (0usize, Vec::new());
    for (p, slot) in work.iter().zip(slots) {
        match slot.into_inner().expect("threads joined") {
            Some(Ok((found, self_kept))) => {
                steady += usize::from(self_kept);
                if !self_kept {
                    failed.push(format!("{}: measured twice without an edit, a library round parted", p.name));
                }
                outcomes.extend(found.into_iter().map(|o| (p.name.clone(), o)));
            }
            Some(Err(e)) => failed.push(format!("{}: {e}", p.name)),
            None => failed.push(format!("{}: not run", p.name)),
        }
    }
    let shown = |s: &str| s.replace(&format!("{}/", kanagawa_root.display()), "");
    print!("{}", report(&outcomes, work.len(), steady, started.elapsed().as_secs_f64()));
    if verbose {
        for (root, o) in &outcomes {
            if let Some(first) = &o.first {
                println!("  {} {}: {}", shown(root), o.kind.name(), shown(first));
            }
            if !o.unequal.is_empty() {
                println!("  {} {}: kept whole, relocated, and ending otherwise: {}", shown(root), o.kind.name(), shown(&o.unequal.join(", ")));
            }
            if !o.earlier.is_empty() {
                println!("  {} {}: state parts first: {}", shown(root), o.kind.name(), shown(&o.earlier.join(", ")));
            }
        }
    }
    let tsv = crate::ptrace::ptrace_dir().with_file_name("loop-rounds.tsv");
    std::fs::write(&tsv, table(&outcomes))?;
    println!("  per edit: {}", tsv.display());
    for f in &failed {
        println!("  FAILED {}", shown(f));
    }
    if !failed.is_empty() {
        anyhow::bail!("{} root(s) not measured", failed.len());
    }
    Ok(())
}

/// One root: the loop before, each edit's loop after, and the root's loop
/// again without an edit — whether that kept every library round.
fn grade_root(settings: &kg_config::Settings, p: &Program, library: &Path) -> Result<(Vec<Outcome>, bool)> {
    let root_path = camino::Utf8PathBuf::from_path_buf(p.cwd.join(&p.file_arg)).map_err(|_| anyhow::anyhow!("a path that is not UTF-8"))?;
    let text = std::fs::read_to_string(&root_path)?;
    let mut db = Db::new(settings.clone());
    let root = db.set_file_text(&root_path, text.clone());
    let root_name = module_name(&db, ModuleSource::File(root));
    let library_edit = crate::edits::library_splice(&db, root, library);
    // The files an edit moves positions in: the root, and the library
    // module `LibraryBody` edits.
    let mut movable = vec![root_name.clone()];
    if let Some((path, _, _)) = &library_edit {
        movable.push(module_name(&db, ModuleSource::File(db.file_id(path))));
    }
    let before = measure(&db, root, &movable)?;
    let again = measure(&db, root, &movable)?;
    let none = Splice { at: 0, removed: 0, inserted: "" };
    let steady = compare(Kind::Body, &before, &again, &root_name, 0, &none);
    let self_kept = steady.share.iter().all(|s| s.rounds[0] == s.rounds[1] && s.nanos[0] == s.nanos[1])
        && steady.finals.iter().all(|c| c.modules[0] == c.modules[1]);
    drop(again);
    let mut out = Vec::new();
    for kind in Kind::ALL {
        let (path, old, splice) = match kind {
            Kind::LibraryBody => match &library_edit {
                Some(target) => target.clone(),
                None => continue,
            },
            _ => match crate::edits::splice(kind, &text) {
                Some(s) => (root_path.clone(), text.clone(), s),
                None => continue,
            },
        };
        let edited = module_name(&db, ModuleSource::File(db.file_id(&path)));
        let index = u8::try_from(movable.iter().position(|m| *m == edited).expect("an edit lands in a movable file"))?;
        db.set_file_text(&path, splice.apply(&old));
        let after = measure(&db, root, &movable)?;
        db.set_file_text(&path, old);
        out.push(compare(kind, &before, &after, &edited, index, &splice));
    }
    Ok((out, self_kept))
}

fn module_name(db: &Db, module: ModuleSource) -> String {
    match module {
        ModuleSource::File(f) => db.path(f).map_or_else(|| format!("{f:?}"), |p| p.to_string()),
        other => format!("{other:?}"),
    }
}

/// The loop over `root`'s program, run to its fixpoint as `kg check` runs
/// it, every round's census spelled and every visit timed.
fn measure(db: &Db, root: FileId, movable: &[String]) -> Result<Run> {
    let sorted = kg_db::sorted::sorted_program(db, &[root]);
    let modules: Vec<ModuleSource> = sorted.files.iter().copied().filter(|m| sorted.ir(*m).is_some()).collect();
    let names: Vec<String> = modules.iter().map(|&m| module_name(db, m)).collect();
    let files: FxHashMap<FileId, String> = modules.iter().zip(&names).map(|(m, n)| (m.diagnostic_file(), n.clone())).collect();
    let irs: Vec<kg_hir::ModuleIr> = modules.iter().map(|m| sorted.ir(*m).expect("filtered").fork()).collect();
    drop(sorted);
    type Rounds = (Vec<Vec<Vec<String>>>, Vec<Vec<Vec<String>>>, Vec<[u64; 3]>, Vec<Vec<Final>>);
    let seen: Rc<RefCell<Rounds>> = Rc::default();
    let observer = {
        let seen = Rc::clone(&seen);
        let names = names.clone();
        let files = files.clone();
        let movable = movable.to_vec();
        move |s: &kg_sem::profile::Seen<'_>| {
            let speller = Speller::new(s.interner, s.expansions, Some(&files));
            let (copies, mentions) = (speller.copies(s.census, s.modules, &names), speller.mentions(s.census, s.modules));
            let states = s.modules.iter().map(|m| speller.final_state(m, &movable)).collect();
            let mut seen = seen.borrow_mut();
            seen.0.push(copies);
            seen.1.push(mentions);
            seen.2.push(size(s.modules));
            seen.3.push(states);
        }
    };
    let full = db.settings().verdicts.infer_full;
    let never = kg_base::cancel::Cancel::never();
    let (out, recordings) = kg_sem::profile::record(observer, || {
        kg_sem::elaborate::elaborate_opts(irs, db.interner(), kg_sem::elaborate::ROUND_GUARD, full, &never)
    });
    let (copies, mentions, sizes, states) = Rc::try_unwrap(seen).map_err(|_| anyhow::anyhow!("the observer outlived the loop"))?.into_inner();
    anyhow::ensure!(recordings.len() == 1, "{} loops recorded, not one", recordings.len());
    anyhow::ensure!(
        copies.len() == out.report.rounds as usize + 1,
        "{} censuses observed over {} rounds",
        copies.len(),
        out.report.rounds
    );
    anyhow::ensure!(!out.report.budget_exhausted, "the loop did not reach its fixpoint");
    let finals = {
        let speller = Speller::new(db.interner(), &out.expansions, Some(&files));
        out.modules.iter().map(|m| speller.final_state(m, movable)).collect()
    };
    let end = size(&out.modules);
    drop(out);
    let mut time = vec![vec![0u64; copies.len() + 1]; names.len()];
    let mut whole = 0;
    for t in &recordings[0].timed {
        let phase = t.round.map_or(0, |r| r as usize + 1);
        match t.module {
            Some(m) => time[m][phase] += t.nanos,
            None => whole += t.nanos,
        }
    }
    Ok(Run { modules: names, copies, mentions, time, whole, finals, states, sizes, end })
}

/// The program's arenas summed: nodes, writes in place, type changes.
fn size(modules: &[kg_sem::Module]) -> [u64; 3] {
    let mut s = [0u64; 3];
    for m in modules {
        s[0] += m.ir.arena.len() as u64;
        s[1] += m.ir.arena.writes();
        s[2] += m.types.changes();
    }
    s
}

/// A range of the old text in the new (`kg_sem::snapshot::Splice::range`).
fn moved(splice: &Splice, start: usize, end: usize) -> Option<(usize, usize)> {
    splice.range(start, end)
}

/// Whether two final states agree, the old one's positions in the movable
/// file `edited` carried through its splice when given.
fn same_final(before: &Final, after: &Final, edited: Option<(u8, &Splice)>) -> bool {
    before.hash == after.hash
        && before.positions.len() == after.positions.len()
        && before.positions.iter().zip(&after.positions).all(|(&(f, s, e), &(g, t, u))| {
            f == g
                && match edited {
                    Some((i, splice)) if f == i => moved(splice, s as usize, e as usize) == Some((t as usize, u as usize)),
                    _ => (s, e) == (t, u),
                }
        })
}

/// `text` with every position in `file` carried through `splice`; one
/// inside what the splice removed is spelled as removed, so it differs from
/// any position.
fn relocate(text: &str, file: &str, splice: &Splice) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        out.push_str(&rest[..open]);
        let inside = &rest[open + OPEN.len_utf8()..];
        let Some(close) = inside.find(CLOSE) else { break };
        let spelled = &inside[..close];
        rest = &inside[close + CLOSE.len_utf8()..];
        let mut parts = spelled.split(SEP);
        let (path, start, end) = (parts.next(), parts.next().and_then(|s| s.parse::<usize>().ok()), parts.next().and_then(|s| s.parse::<usize>().ok()));
        match (path, start, end) {
            (Some(p), Some(s), Some(e)) if p == file => {
                match moved(splice, s, e) {
                    Some((s, e)) => out.push_str(&position(p, s, e)),
                    None => out.push_str(&format!("{OPEN}{p}{SEP}removed{CLOSE}")),
                }
            }
            _ => {
                out.push(OPEN);
                out.push_str(spelled);
                out.push(CLOSE);
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` without its positions.
fn unplaced(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        out.push_str(&rest[..open]);
        match rest[open..].find(CLOSE) {
            Some(close) => rest = &rest[open + close + CLOSE.len_utf8()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` with its positions as `@path:start..end`, to read.
fn readable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut seps = 0;
    for c in text.chars() {
        match c {
            OPEN => {
                out.push('@');
                seps = 0;
            }
            SEP => {
                out.push_str(if seps == 0 { ":" } else { ".." });
                seps += 1;
            }
            CLOSE => {}
            c => out.push(c),
        }
    }
    out
}

/// Module `m`'s input to round `round` of `run` under `reading`, positions
/// in `relocated`'s file carried through its splice when given.
fn input(run: &Run, m: usize, round: usize, reading: Reading, relocated: Option<(&str, &Splice)>) -> Vec<String> {
    let mut v: Vec<String> = run.copies[round][m].clone();
    if reading == Reading::Mentions {
        v.extend(run.mentions[round][m].iter().map(|s| format!("mention {s}")));
    }
    match (reading, relocated) {
        (Reading::Written, _) | (_, None) => v,
        (_, Some((file, splice))) => v.iter().map(|s| relocate(s, file, splice)).collect(),
    }
}

/// The first round of `after`'s module `ma` whose input differs from
/// `before`'s module `mb`'s — a round only one run has, or the last of
/// one run and not the other's (its catch-all differs), included — or
/// `after`'s round count when none does.
fn first_difference(before: &Run, mb: usize, after: &Run, ma: usize, edited: &str, splice: &Splice, reading: Reading) -> usize {
    let (nb, na) = (before.copies.len(), after.copies.len());
    (0..na)
        .find(|&r| {
            r >= nb
                || (r + 1 == nb) != (r + 1 == na)
                || input(before, mb, r, reading, Some((edited, splice))) != input(after, ma, r, reading, None)
        })
        .unwrap_or(na)
}

/// One edit's runs, compared module by module.
fn compare(kind: Kind, before: &Run, after: &Run, edited: &str, index: u8, splice: &Splice) -> Outcome {
    let mut share = [Share::default(); 4];
    let mut finals = [Check::default(); 2];
    let na = after.copies.len();
    let mut library_modules = 0;
    let (mut edited_ns, mut library_ns) = (0u64, 0u64);
    let mut first: Option<(usize, String)> = None;
    let mut unequal = Vec::new();
    let mut earlier = Vec::new();
    for (ma, name) in after.modules.iter().enumerate() {
        let t = &after.time[ma];
        let all: u64 = t.iter().sum();
        if name == edited {
            edited_ns += all;
            continue;
        }
        library_modules += 1;
        library_ns += all;
        let mb = before.modules.iter().position(|n| n == name);
        for (reading, s) in Reading::ALL.into_iter().zip(&mut share) {
            // A module the old run did not hold has nothing kept, not even
            // its passes before the loop; nor, state checked, one whose
            // state at the first census differs.
            let kept = match (reading, mb) {
                (_, None) => None,
                (Reading::State, Some(mb)) => {
                    let k = first_difference(before, mb, after, ma, edited, splice, Reading::Relocated);
                    let moved = Some((index, splice));
                    let state = match (0..k).find(|&r| !same_final(&before.states[r][mb], &after.states[r][ma], moved)) {
                        Some(0) => None,
                        Some(r) => Some(r),
                        None if k == na && !same_final(&before.finals[mb], &after.finals[ma], moved) => Some(na - 1),
                        None => Some(k),
                    };
                    if state != Some(k) {
                        earlier.push(format!("{name} (state in round {}, copies in {k})", state.map_or("before 0".to_owned(), |r| r.to_string())));
                    }
                    state
                }
                (_, Some(mb)) => Some(first_difference(before, mb, after, ma, edited, splice, reading)),
            };
            let sum = |rounds: usize| t[0] + t[1..=rounds].iter().sum::<u64>();
            let (k, k4) = kept.map_or((0, 0), |k| (k, k.min(EDITOR_ROUNDS)));
            s.add(&Share {
                rounds: [k as u64, na as u64],
                nanos: [kept.map_or(0, |_| sum(k)), all],
                rounds4: [k4 as u64, na.min(EDITOR_ROUNDS) as u64],
                nanos4: [kept.map_or(0, |_| sum(k4)), sum(na.min(EDITOR_ROUNDS))],
            });
            // Kept whole: does the module end where it ended before?
            if let (Some(mb), Some(k)) = (mb, kept)
                && k == na
                && matches!(reading, Reading::Written | Reading::Relocated)
            {
                let moved = (reading == Reading::Relocated).then_some((index, splice));
                let same = same_final(&before.finals[mb], &after.finals[ma], moved);
                let c = &mut finals[usize::from(reading == Reading::Relocated)];
                c.modules[0] += 1;
                c.nanos[0] += all;
                if same {
                    c.modules[1] += 1;
                    c.nanos[1] += all;
                } else if reading == Reading::Relocated {
                    unequal.push(name.clone());
                }
            }
            let at = kept.unwrap_or(0);
            if reading == Reading::Relocated && at < na && first.as_ref().is_none_or(|(f, _)| at < *f) {
                let why = match mb {
                    Some(mb) => difference(&input(before, mb, at, reading, Some((edited, splice))), &input(after, ma, at, reading, None)),
                    None => "new".to_owned(),
                };
                first = Some((at, format!("{name} parts in round {at}: {why}")));
            }
        }
    }
    let requests = {
        let of = |run: &Run| run.modules.iter().position(|n| n == edited).map(|m| run.mentions.first().map(|r| r[m].clone()).unwrap_or_default());
        match (of(before), of(after)) {
            (Some(b), Some(a)) => {
                let moved: Vec<String> = b.iter().map(|s| relocate(s, edited, splice)).collect();
                let asked = |v: &[String]| {
                    let mut v: Vec<String> = v.iter().map(|s| unplaced(s)).collect();
                    v.sort();
                    v
                };
                [b != a, moved != a, asked(&b) != asked(&a)]
            }
            _ => [true; 3],
        }
    };
    let whole_ns = after.whole;
    Outcome {
        kind,
        rounds: (before.copies.len(), na),
        library_modules,
        share,
        finals,
        requests,
        loop_ns: edited_ns + library_ns + whole_ns,
        edited_ns,
        library_ns,
        whole_ns,
        first: first.map(|(_, s)| s),
        unequal,
        earlier,
        sizes: [after.sizes.first().copied().unwrap_or_default(), after.end],
    }
}

/// The first entry two inputs differ at, both sides, cut short.
fn difference(before: &[String], after: &[String]) -> String {
    let cut = |s: &str| readable(s).replace('\n', " | ").chars().take(240).collect::<String>();
    match before.iter().zip(after).position(|(b, a)| b != a) {
        Some(i) => {
            let (b, a) = (&before[i], &after[i]);
            let line = b.lines().zip(a.lines()).position(|(x, y)| x != y).unwrap_or(0);
            let (bl, al) = (b.lines().nth(line).unwrap_or(""), a.lines().nth(line).unwrap_or(""));
            format!("entry {i}, line {line}: `{}` against `{}` (copy `{}`)", cut(bl), cut(al), cut(a.lines().next().unwrap_or("")))
        }
        None => format!(
            "{} entries against {}{}",
            before.len(),
            after.len(),
            after.get(before.len()).or(before.get(after.len())).map(|s| format!(", first unmatched `{}`", cut(s.lines().next().unwrap_or("")))).unwrap_or_default()
        ),
    }
}

fn fraction(x: [u64; 2]) -> f64 {
    if x[1] == 0 { 1.0 } else { x[0] as f64 / x[1] as f64 }
}

/// The value at quantile `q` of sorted `v`, by nearest rank.
fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let i = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[i]
}

/// The summary: per reading, per edit kind and over the corpus.
fn report(outcomes: &[(String, Outcome)], roots: usize, steady: usize, seconds: f64) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let pct = |x: f64| format!("{:5.1}%", 100.0 * x);
    let _ = writeln!(
        out,
        "loop-rounds: {roots} roots, {} edits, each run to the fixpoint before and after ({seconds:.1}s)",
        outcomes.len()
    );
    let _ = writeln!(out, "  measured twice without an edit, every library round kept on {steady}/{roots} roots");
    let groups: Vec<(&str, Vec<&Outcome>)> = Kind::ALL
        .iter()
        .map(|k| (k.name(), outcomes.iter().map(|(_, o)| o).filter(|o| o.kind == *k).collect()))
        .chain(std::iter::once(("all", outcomes.iter().map(|(_, o)| o).collect())))
        .collect();
    let _ = writeln!(out, "  after the edit, the loop's time: the library's per-module passes / the edited module's / whole-program steps");
    for (name, os) in &groups {
        let total: u64 = os.iter().map(|o| o.loop_ns).sum();
        let part = |f: fn(&Outcome) -> u64| fraction([os.iter().map(|o| f(o)).sum(), total]);
        let _ = writeln!(
            out,
            "    {name:<13} {} / {} / {}   ({:.1} s of loops, rounds after {:.1} on average)",
            pct(part(|o| o.library_ns)),
            pct(part(|o| o.edited_ns)),
            pct(part(|o| o.whole_ns)),
            total as f64 / 1e9,
            os.iter().map(|o| o.rounds.1 as f64).sum::<f64>() / os.len().max(1) as f64
        );
    }
    let _ = writeln!(
        out,
        "  the edited module's own occurrences differ in round 0 (as written / relocated / what they ask for)"
    );
    for (name, os) in &groups {
        let n = os.len().max(1) as f64;
        let share = |i: usize| os.iter().filter(|o| o.requests[i]).count() as f64 / n;
        let _ = writeln!(out, "    {name:<13} {} / {} / {}", pct(share(0)), pct(share(1)), pct(share(2)));
    }
    {
        // What D1 would hold: the last run's program, and what rewrites
        // it in place after the first round (its undo logs).
        let mut v: Vec<[u64; 3]> = outcomes.iter().map(|(_, o)| {
            let [r0, end] = o.sizes;
            [end[0], end[1].saturating_sub(r0[1]), end[2].saturating_sub(r0[2])]
        }).collect();
        let pick = |v: &mut Vec<[u64; 3]>, i: usize| -> (u64, u64) {
            v.sort_by_key(|x| x[i]);
            (v.get(v.len() / 2).map_or(0, |x| x[i]), v.last().map_or(0, |x| x[i]))
        };
        let (nodes, writes, changes) = (pick(&mut v, 0), pick(&mut v, 1), pick(&mut v, 2));
        let _ = writeln!(
            out,
            "  after the edit, at the end (median / max): {} / {} nodes; after round 0's census, {} / {} written in place, {} / {} types changed ({} bytes a node, {} a type)",
            nodes.0,
            nodes.1,
            writes.0,
            writes.1,
            changes.0,
            changes.1,
            std::mem::size_of::<kg_hir::arena::NodeData>(),
            std::mem::size_of::<kg_ty::Ty>()
        );
    }
    let _ = writeln!(
        out,
        "  library modules kept whole that end as they ended before — as written; relocated (modules, time)"
    );
    for (name, os) in &groups {
        let mut c = [Check::default(); 2];
        for o in os {
            for (sum, x) in c.iter_mut().zip(&o.finals) {
                for i in 0..2 {
                    sum.modules[i] += x.modules[i];
                    sum.nanos[i] += x.nanos[i];
                }
            }
        }
        let _ = writeln!(
            out,
            "    {name:<13} {}/{} {};  {}/{} {}",
            c[0].modules[1],
            c[0].modules[0],
            pct(fraction([c[0].nanos[1], c[0].nanos[0]])),
            c[1].modules[1],
            c[1].modules[0],
            pct(fraction([c[1].nanos[1], c[1].nanos[0]]))
        );
    }
    for (r, reading) in Reading::ALL.iter().enumerate() {
        let _ = writeln!(out, "  reading {:?}: library work kept — module-rounds, time pooled; time per edit: q1 / median / q3, edits keeping more than half", reading.name());
        for (label, four) in [("every round", false), ("rounds 0–3", true)] {
            let _ = writeln!(out, "    {label}");
            for (name, os) in &groups {
                let mut total = Share::default();
                for o in os {
                    total.add(&o.share[r]);
                }
                let (rounds, nanos) = if four { (total.rounds4, total.nanos4) } else { (total.rounds, total.nanos) };
                let mut per: Vec<f64> = os
                    .iter()
                    .filter(|o| o.library_modules > 0)
                    .map(|o| fraction(if four { o.share[r].nanos4 } else { o.share[r].nanos }))
                    .collect();
                per.sort_by(f64::total_cmp);
                let over = per.iter().filter(|&&x| x > 0.5).count() as f64 / per.len().max(1) as f64;
                let _ = writeln!(
                    out,
                    "      {name:<13} {} {}   {} / {} / {}  {}",
                    pct(fraction(rounds)),
                    pct(fraction(nanos)),
                    pct(quantile(&per, 0.25)),
                    pct(quantile(&per, 0.5)),
                    pct(quantile(&per, 0.75)),
                    pct(over)
                );
            }
        }
    }
    out
}

/// One row per edit, tab-separated, for reading again.
fn table(outcomes: &[(String, Outcome)]) -> String {
    let mut out = String::from(
        "root\tkind\trounds_before\trounds_after\tlibrary_modules\tloop_ns\tedited_ns\tlibrary_ns\twhole_ns\trequests_written\trequests_relocated\trequests_asked\
         \twhole_written\tsame_written\twhole_written_ns\tsame_written_ns\twhole_relocated\tsame_relocated\twhole_relocated_ns\tsame_relocated_ns\
         \tnodes_r0\twrites_r0\tchanges_r0\tnodes_end\twrites_end\tchanges_end",
    );
    for r in Reading::ALL {
        let n = r.name().replace([' ', ','], "_").replace("__", "_");
        for f in ["rounds_kept", "rounds_all", "ns_kept", "ns_all", "rounds4_kept", "rounds4_all", "ns4_kept", "ns4_all"] {
            out.push_str(&format!("\t{n}:{f}"));
        }
    }
    out.push('\n');
    for (root, o) in outcomes {
        out.push_str(&format!(
            "{root}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            o.kind.name(),
            o.rounds.0,
            o.rounds.1,
            o.library_modules,
            o.loop_ns,
            o.edited_ns,
            o.library_ns,
            o.whole_ns,
            u8::from(o.requests[0]),
            u8::from(o.requests[1]),
            u8::from(o.requests[2])
        ));
        for c in &o.finals {
            out.push_str(&format!("\t{}\t{}\t{}\t{}", c.modules[0], c.modules[1], c.nanos[0], c.nanos[1]));
        }
        for x in o.sizes.iter().flatten() {
            out.push_str(&format!("\t{x}"));
        }
        for s in &o.share {
            for x in [s.rounds, s.nanos, s.rounds4, s.nanos4] {
                out.push_str(&format!("\t{}\t{}", x[0], x[1]));
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::Utf8PathBuf;

    const LIB: &str = "module lib { Box, id }\n\ntemplate <typename T>\nstruct Box\n{\n    T x;\n}\n\n\
                       template <typename T>\ninline T id(T x)\n{\n    return x;\n}\n";
    const MAIN: &str = "import lib\n\ninline void g()\n{\n}\n\n\
                        inline uint8 f(uint8 y)\n{\n    Box<uint8> b;\n    return id<uint8>(y);\n}\n";

    /// `main` and the library it imports, in a `Db` of their own: the
    /// database, the root, the files an edit may move.
    fn program(main: &str) -> (Db, FileId, Vec<String>) {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = Db::new(settings);
        db.set_file_text(&Utf8PathBuf::from("/proj/lib.k"), LIB.to_owned());
        let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
        (db, root, vec!["/proj/main.k".to_owned()])
    }

    /// The loop before and after `splice` is made to `MAIN`, compared.
    fn edited(splice: &Splice) -> Outcome {
        let (mut db, root, movable) = program(MAIN);
        let before = measure(&db, root, &movable).expect("measured");
        db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), splice.apply(MAIN));
        let after = measure(&db, root, &movable).expect("measured");
        assert!(after.copies.len() > 1, "the library is asked for copies: {:?}", after.copies);
        compare(Kind::Body, &before, &after, "/proj/main.k", 0, splice)
    }

    /// The same program measured twice keeps every library round, and
    /// every library module ends as it ended — so nothing a run mints (an
    /// arena's stamp, an expansion's number) is spelled. Red with a copy
    /// spelled with the `ExpId` of its occurrence, or the final state left
    /// stamped.
    #[test]
    fn the_same_program_twice_keeps_every_library_round() {
        let (db, root, movable) = program(MAIN);
        let a = measure(&db, root, &movable).expect("measured");
        let b = measure(&db, root, &movable).expect("measured");
        assert!(a.copies.iter().flatten().any(|m| !m.is_empty()), "copies were decided: {:?}", a.copies);
        let none = Splice { at: 0, removed: 0, inserted: "" };
        let o = compare(Kind::Body, &a, &b, "/proj/main.k", 0, &none);
        for s in &o.share {
            assert_eq!(s.rounds[0], s.rounds[1], "{o:?}");
            assert_eq!(s.nanos[0], s.nanos[1], "{o:?}");
        }
        assert!(o.finals.iter().all(|c| c.modules[0] == o.library_modules as u64 && c.modules[1] == c.modules[0]), "{o:?}");
        assert_eq!(o.requests, [false; 3]);
    }

    /// A statement added above the root's requests moves them and asks for
    /// nothing new: as written, the library's first round differs (the
    /// copies' sites and arguments moved); relocated, it is kept whole and
    /// ends as it ended. Red with the positions not carried through the
    /// splice.
    #[test]
    fn an_edit_that_asks_for_nothing_new_keeps_the_library_once_relocated() {
        let splice = crate::edits::splice(Kind::Body, MAIN).expect("a body");
        assert!(splice.at < MAIN.find("Box<uint8>").expect("a request"), "the edit lands above the requests");
        let o = edited(&splice);
        let [written, relocated, mentions, state] = o.share;
        // `lib.k` keeps none of its rounds as written: two of four
        // module-rounds, the other module's.
        assert_eq!(written.rounds, [2, 4], "{o:?}");
        assert_eq!(relocated.rounds, [4, 4], "{o:?}");
        assert_eq!(mentions.rounds, [4, 4], "{o:?}");
        assert_eq!(state.rounds, [4, 4], "{o:?}");
        assert_eq!(o.finals[1].modules, [2, 2], "{o:?}");
        assert_eq!(o.first, None);
        assert_eq!(o.requests, [true, false, false]);
    }

    /// An edit that asks for another copy parts the library in the round
    /// the copy is decided, under every reading. Red with a copy filed
    /// under the module that asked for it rather than the template's.
    #[test]
    fn an_edit_that_asks_for_another_copy_parts_the_library() {
        let at = MAIN.find("id<uint8>").expect("a request") + "id<".len();
        let splice = Splice { at, removed: "uint8".len(), inserted: "uint16" };
        let o = edited(&splice);
        for s in &o.share {
            assert_eq!(s.rounds, [2, 4], "{o:?}");
        }
        let first = o.first.as_deref().unwrap_or_default();
        assert!(first.starts_with("/proj/lib.k parts in round 0"), "{first}");
        assert_eq!(o.requests, [true, true, true]);
    }

    /// A class's method, called from a library template's copy over the
    /// class, is typed there through the symbol map, not the template's
    /// arguments (a class's type holds its fields only): a signature edit
    /// to the method leaves every copy's input as it was and changes the
    /// copy's state anyway — what §3's (3) covers and the copies cannot see
    /// (`library/test/unit.k`'s `fixture<Test100>` under the corpus's
    /// signature edits). The copies-only readings keep the library whole;
    /// the state-checked one does not, and the final state says so. Red
    /// with the states never compared.
    #[test]
    fn a_read_across_the_boundary_parts_the_state_and_not_the_copies() {
        const CALLER: &str = "import lib\n\nclass K\n{\npublic:\n    void run(uint8 tag)\n    {\n    }\n}\n\n\
                              inline void f()\n{\n    call<K>();\n}\n";
        // `inner<K>` is decided inside the copy, a round later: the state
        // parts in round 1, before the loop's last.
        const CALLED: &str = "module lib { call }\n\ntemplate <typename F>\ninline void inner()\n{\n}\n\n\
                              template <typename F>\ninline void call()\n{\n    static F test;\n    test.run(1);\n    inner<F>();\n}\n";
        let (mut db, root, movable) = program(CALLER);
        db.set_file_text(&Utf8PathBuf::from("/proj/lib.k"), CALLED.to_owned());
        let before = measure(&db, root, &movable).expect("measured");
        let splice = crate::edits::splice(Kind::Signature, CALLER).expect("a signature");
        db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), splice.apply(CALLER));
        let after = measure(&db, root, &movable).expect("measured");
        let o = compare(Kind::Signature, &before, &after, "/proj/main.k", 0, &splice);
        let [_, relocated, _, state] = o.share;
        assert_eq!(o.rounds, (3, 3));
        assert_eq!(relocated.rounds, [6, 6], "{o:?}");
        // `lib.k` keeps round 0 of three; the other module all of its.
        assert_eq!(state.rounds, [1 + 3, 6], "{o:?}");
        assert_eq!(o.unequal, ["/proj/lib.k"]);
        assert_eq!(o.earlier, ["/proj/lib.k (state in round 1, copies in 3)"]);
    }

    fn run(modules: &[&str], copies: &[&[&str]], time: &[&[u64]]) -> Run {
        let rounds = copies.len();
        Run {
            modules: modules.iter().map(|m| (*m).to_owned()).collect(),
            copies: (0..rounds)
                .map(|r| (0..modules.len()).map(|m| if copies[r][m].is_empty() { Vec::new() } else { vec![copies[r][m].to_owned()] }).collect())
                .collect(),
            mentions: vec![vec![Vec::new(); modules.len()]; rounds],
            time: time.iter().map(|t| t.to_vec()).collect(),
            whole: 5,
            finals: vec![Final::default(); modules.len()],
            states: vec![vec![Final::default(); modules.len()]; rounds],
            sizes: vec![[0; 3]; rounds],
            end: [0; 3],
        }
    }

    /// The accounting: a library module's rounds before its first differing
    /// one are kept, with its time before the loop; a module the old run
    /// did not hold keeps nothing; the edited module is no library's; and
    /// rounds 0–3 are the first four. Red with a new module's passes before
    /// the loop counted as kept, or the edited module counted.
    #[test]
    fn rounds_before_the_first_difference_are_kept() {
        let none = Splice { at: 0, removed: 0, inserted: "" };
        // Five rounds; `a` parts in round 2, `b` never, `n` is new.
        let before = run(
            &["a", "b", "root"],
            &[&["x", "", "r"], &["", "y", ""], &["z", "", ""], &["", "", ""], &["", "", ""]],
            &[&[0; 6], &[0; 6], &[0; 6]],
        );
        let after = run(
            &["a", "b", "n", "root"],
            &[&["x", "", "", "r2"], &["", "y", "", ""], &["w", "", "", ""], &["", "", "", ""], &["", "", "", ""]],
            &[&[1, 2, 4, 8, 16, 32], &[1, 1, 1, 1, 1, 1], &[3, 3, 3, 3, 3, 3], &[100, 100, 100, 100, 100, 100]],
        );
        let o = compare(Kind::Body, &before, &after, "root", 0, &none);
        assert_eq!(o.library_modules, 3);
        assert_eq!(o.edited_ns, 600);
        assert_eq!(o.library_ns, 63 + 6 + 18);
        assert_eq!(o.loop_ns, 600 + 87 + 5);
        let s = o.share[0];
        assert_eq!(s.rounds, [2 + 5, 15]);
        assert_eq!(s.nanos, [(1 + 2 + 4) + 6, 87]);
        assert_eq!(s.rounds4, [2 + 4, 12]);
        assert_eq!(s.nanos4, [(1 + 2 + 4) + 5, (1 + 2 + 4 + 8 + 16) + 5 + 15]);
        assert_eq!(o.finals[0].modules, [1, 1], "`b`, kept whole");
    }
}
