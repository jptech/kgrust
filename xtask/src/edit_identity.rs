//! `cargo xtask edit-identity`: a warm `Db` after an edit against a fresh
//! one over the edited text, every corpus root, every kind of edit and its
//! undo (sprint W7.1).
//!
//! It is to the query layer what `infer-identity` is to incremental
//! inference. Each memo is keyed on the inputs someone *listed*; a key that
//! misses one serves yesterday's answer, and nothing else in the gates
//! would see it — every other gate runs a fresh `Db` per program. So each
//! root is loaded into a `Db` that stays warm across the corpus, as an
//! editor's does; each of `crate::edits::Kind` is made in its overlay and
//! then undone; and after every step these are compared with a fresh
//! `Db`'s over the same texts:
//!
//! * the diagnostics `Db::check` returns;
//! * the loop's fingerprint (`kg_sem::elaborate::fingerprint`) — every
//!   module's final tree, every node's type, the report;
//! * **the editor's diagnostics** (sprint W7.6, W7 item 6's exit): the warm
//!   `Db` is the editor's, under its layered budget (`PassBudget::EDITOR`),
//!   and the root's diagnostics as the server publishes them
//!   (`Db::diagnostics_layered`) are read before anything else is asked.
//!   Their last answer must be `kg check`'s — the fresh `Db`'s
//!   `Db::check`, at the root's file — and their first, where the loop ran
//!   past four rounds, must show nothing the last does not: it may miss a
//!   diagnostic, never gain one. The one difference by design is the
//!   editor's parse gate, per file where `kg check`'s is program-wide
//!   (`Db::diagnostics`): where another module's parse stage errs, the
//!   step is counted apart and not compared.
//!
//! The first two are then read off the editor's database too, so its
//! layered loop — a run on a thread of its own, its answers memoized as
//! they land (`kg_db::layered`) — is graded by every one of them. Until
//! sprint W3.7 every module's CST type table was compared too, entry by
//! entry; it went with the CST inference engine.
//!
//! **The snapshot's shadow** (sprint W7.5d, D1 step 1; `kg_db::snapshot`):
//! the editor keeps each run's state per module and round, and here every
//! step after the first checks the step before's against its own run —
//! each module whose lowering is the kept run's, each round through which
//! its key (the copies decided into it, positions carried through the
//! edit) agrees, restored and compared node by node with the round it ran.
//! Its ledger is `shadow`: `kept`, the steps whose every followed round's
//! key agreed (a pass set), and `unequal`, each module whose restored state
//! is not the actual at its first such round (a fail set). An unfiltered
//! run writes it; a sampled one (`--every`, `--filter`) is held to the
//! baseline's items for the roots it ran.
//!
//! File ids and symbols are spelled out before comparing — two `Db`s intern
//! in different orders — and a difference names the part, and the module
//! or the first differing line.
//!
//! Green today by construction: nothing is skipped yet, and a warm `Db`
//! recomputes what an edit touched. It exists so that each mechanism of W7
//! — a lowering memo, the interface firewall — lands after the check that
//! can see it go wrong, with the mutant that leaves an input out of its key
//! shown red first.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use kg_base::FileId;
use kg_db::Db;

use crate::edits::Kind;
use crate::ptrace::{programs, Program};

/// One root's differences, one line each, the edits that had somewhere to
/// land, and what the editor's row saw.
#[derive(Default)]
struct Graded {
    found: Vec<String>,
    made: Vec<Kind>,
    editor: EditorCounts,
    /// Each step's shadow report: the step, and what it found.
    shadow: Vec<(String, kg_sem::snapshot::ShadowReport)>,
    /// What the editor kept after each step (`kg_db::snapshot`).
    kept: Vec<kg_sem::snapshot::Size>,
    /// Each step's reuse of the run before (`kg_sem::reuse`).
    reused: Vec<(String, kg_sem::reuse::ReuseReport)>,
}

/// The shadow's row, over the steps of one kind or all of them.
#[derive(Default, Clone, Copy)]
struct ShadowCounts {
    steps: usize,
    kept: usize,
    comparable: u64,
    whole: u64,
    rounds: u64,
    agreed: u64,
    /// Module-rounds whose copies agreed and whose reads did not, and of
    /// those the ones restored equal all the same.
    read_parted: u64,
    read_parted_equal: u64,
    equal: u64,
    unequal: usize,
}

impl ShadowCounts {
    fn add(&mut self, r: &kg_sem::snapshot::ShadowReport) {
        self.steps += 1;
        self.kept += usize::from(r.agreed == r.rounds);
        self.comparable += r.comparable as u64;
        self.whole += r.whole as u64;
        self.rounds += r.rounds;
        self.agreed += r.agreed;
        self.read_parted += r.read_parted;
        self.read_parted_equal += r.read_parted_equal;
        self.equal += r.equal;
        self.unequal += r.mismatches.len();
    }
}

/// The editor's row, over the steps of one root or all of them.
#[derive(Default, Clone, Copy)]
struct EditorCounts {
    steps: usize,
    /// Steps whose last answer was compared with `kg check`'s.
    compared: usize,
    /// Steps another module's parse stage stopped `kg check` at.
    gated: usize,
    /// Steps with a first answer: the loop ran past the editor's bound.
    firsts: usize,
}

impl EditorCounts {
    fn add(&mut self, o: EditorCounts) {
        self.steps += o.steps;
        self.compared += o.compared;
        self.gated += o.gated;
        self.firsts += o.firsts;
    }
}

/// Everything compared, spelled independently of the `Db`.
struct State {
    diagnostics: String,
    fingerprint: String,
    /// `Db::check`'s diagnostics in the root's file, each with its start.
    root_file: Vec<(u32, String)>,
    /// Whether the parse stage erred in a module other than the root's,
    /// where `kg check` stops and the editor's per-file gate does not.
    gated_elsewhere: bool,
}

/// The editor's answers for the root's file, as the server publishes them
/// (`Db::diagnostics_layered`), each diagnostic with its start: the first
/// where there is one, and the last.
struct Editor {
    first: Option<Vec<(u32, String)>>,
    last: Vec<(u32, String)>,
}

pub fn run(kanagawa_root: &Path, filter: Option<&str>, every: usize) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let started = std::time::Instant::now();
    let mut work: Vec<Program> = programs(kanagawa_root)?
        .into_iter()
        .filter(|p| p.name.starts_with("file::") && filter.is_none_or(|f| p.name.contains(f)))
        .collect();
    if every > 1 {
        work = work.into_iter().step_by(every).collect();
    }
    // The largest first: one root (`risc-v.k`) costs a minute alone, and a
    // queue that reaches it last finishes a minute after everything else.
    let size = |p: &Program| std::fs::metadata(p.cwd.join(&p.file_arg)).map_or(0, |m| m.len());
    work.sort_by_key(|p| std::cmp::Reverse(size(p)));
    if work.is_empty() {
        anyhow::bail!("no root matched");
    }
    let library = kanagawa_root.join("library");
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(6);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Graded>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // Warm across roots, as an editor's is, and under the
                // editor's budget.
                let mut warm: Option<(Db, Vec<String>)> = None;
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(p) = work.get(i) else { break };
                    let settings = crate::conformance::settings_for(kanagawa_root, &p.options);
                    let (db, options) = warm.get_or_insert_with(|| {
                        let mut db = Db::new(settings.clone());
                        db.set_pass_budget(kg_db::PassBudget::EDITOR);
                        db.set_snapshot_shadow(true);
                        db.set_reuse(true, true);
                        (db, p.options.clone())
                    });
                    if *options != p.options {
                        db.set_settings(settings.clone());
                        *options = p.options.clone();
                    }
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        grade_root(db, &settings, p, &library)
                    }))
                    .unwrap_or_else(|e| {
                        let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()));
                        Graded { found: vec![format!("panic: {}", what.unwrap_or_default())], ..Graded::default() }
                    });
                    if outcome.found.iter().any(|d| d.starts_with("panic")) {
                        warm = None;
                    }
                    *slots[i].lock().expect("no panics hold this lock") = Some(outcome);
                }
            });
        }
    });
    let mut differ = 0usize;
    let mut lines: Vec<String> = Vec::new();
    let mut made = [0usize; Kind::ALL.len()];
    let mut editor = EditorCounts::default();
    let mut ledger = crate::ratchet::Ledger::new("shadow");
    ledger.set("kept");
    ledger.set("unequal");
    ledger.set("reuse-unequal");
    let mut reuse = kg_sem::reuse::ReuseReport::default();
    let mut shadow: Vec<(String, ShadowCounts)> = Vec::new();
    let mut sizes: Vec<kg_sem::snapshot::Size> = Vec::new();
    let mut graded_roots = std::collections::BTreeSet::new();
    let shown = |s: &str| s.replace(&format!("{}/", kanagawa_root.display()), "");
    for (p, slot) in work.iter().zip(slots) {
        let Graded { found, made: kinds, editor: counts, shadow: reports, kept, reused: reuses } =
            slot.into_inner().expect("threads joined").unwrap_or_default();
        for (step, r) in &reuses {
            reuse.offered += r.offered;
            reuse.reused += r.reused;
            reuse.nodes += r.nodes;
            reuse.checked += r.checked;
            for u in &r.unequal {
                ledger.add("reuse-unequal", &format!("{}|{step}|{}", p.name, shown(u)));
                lines.push(format!("  REUSE {}: {step}: {}", p.name, shown(u)));
                reuse.unequal.push(u.clone());
            }
        }
        sizes.extend(kept);
        editor.add(counts);
        graded_roots.insert(p.name.clone());
        for (step, r) in &reports {
            let kind = step.trim_end_matches(" undone");
            for key in [kind, "all"] {
                match shadow.iter_mut().find(|(k, _)| k == key) {
                    Some((_, c)) => c.add(r),
                    None => {
                        let mut c = ShadowCounts::default();
                        c.add(r);
                        shadow.push((key.to_owned(), c));
                    }
                }
            }
            if r.agreed == r.rounds {
                ledger.add("kept", &format!("{}|{step}", p.name));
            }
            for m in &r.mismatches {
                ledger.add("unequal", &format!("{}|{step}|{}@{}", p.name, shown(&m.name), m.round));
                lines.push(format!("  SHADOW {}: {step}: {} round {}: {}", p.name, shown(&m.name), m.round, m.what));
            }
        }
        for k in kinds {
            made[Kind::ALL.iter().position(|&x| x == k).expect("listed")] += 1;
        }
        if !found.is_empty() {
            differ += 1;
            for d in found {
                lines.push(format!("  DIFFER {}: {d}", p.name));
            }
        }
    }
    println!(
        "edit-identity: {} roots × {} edits and their undo in {:.1}s — {} identical, {differ} differ",
        work.len(),
        Kind::ALL.len(),
        started.elapsed().as_secs_f64(),
        work.len() - differ
    );
    println!(
        "  edits made: {}",
        Kind::ALL.iter().zip(made).map(|(k, n)| format!("{} {n}", k.name())).collect::<Vec<_>>().join(", ")
    );
    println!(
        "  the editor: {} steps, its last answer `kg check`'s on {} (the per-file parse gate apart on {}); \
         a first answer on {}, each within its last",
        editor.steps, editor.compared, editor.gated, editor.firsts
    );
    if let Some((_, all)) = shadow.iter().find(|(k, _)| k == "all") {
        println!(
            "  the snapshot's shadow: {} steps after an edit, {} kept whole; keys agreed in {} of {} module-rounds ({:.1}%), \
             restored equal {} of them; {} unequal; {} parted by what they read alone, {} of those equal all the same",
            all.steps,
            all.kept,
            all.agreed,
            all.rounds,
            100.0 * all.agreed as f64 / all.rounds.max(1) as f64,
            all.equal,
            all.unequal,
            all.read_parted,
            all.read_parted_equal
        );
        for (kind, c) in &shadow {
            println!(
                "    {kind:<20} {} steps, {} kept whole; {} of {} modules followed whole; keys agreed {}/{} ({:.1}%), equal {}; \
                 parted by reads {} ({} equal)",
                c.steps,
                c.kept,
                c.whole,
                c.comparable,
                c.agreed,
                c.rounds,
                100.0 * c.agreed as f64 / c.rounds.max(1) as f64,
                c.equal,
                c.read_parted,
                c.read_parted_equal
            );
        }
        // What the editor holds for the next run: the program, shared
        // with its last answer, and beside it the logs and the keys.
        let spread = |f: &dyn Fn(&kg_sem::snapshot::Size) -> u64| -> String {
            let mut v: Vec<u64> = sizes.iter().map(f).collect();
            v.sort_unstable();
            let at = |q: f64| v.get(((v.len() as f64 * q) as usize).min(v.len().saturating_sub(1))).copied().unwrap_or(0);
            format!("{} / {} / {}", at(0.5), at(0.9), v.last().copied().unwrap_or(0))
        };
        println!(
            "  kept after each step (median / p90 / max of {}): program nodes {}; logged nodes {}, types {}; key positions {}; \
             read positions {}; bytes beside the program {}",
            sizes.len(),
            spread(&|s| s.nodes),
            spread(&|s| s.logged_nodes),
            spread(&|s| s.logged_types),
            spread(&|s| s.key_positions),
            spread(&|s| s.read_positions),
            spread(&|s| s.bytes)
        );
        let total = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
        ledger.total("graded", all.steps);
        ledger.total("rounds", total(all.rounds));
        ledger.total("agreed", total(all.agreed));
        ledger.total("equal", total(all.equal));
        ledger.total("whole", total(all.whole));
        ledger.total("read_parted", total(all.read_parted));
        ledger.total("read_parted_equal", total(all.read_parted_equal));
    }
    // What the editor reused of each run before (D1 step 3), every pass
    // reused also run on a fork and compared.
    println!(
        "  the prefix reused: {} of {} module-rounds offered ({} nodes taken in); checked {}, unequal {}",
        reuse.reused,
        reuse.offered,
        reuse.nodes,
        reuse.checked,
        reuse.unequal.len()
    );
    let total = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
    ledger.total("prefix_offered", total(reuse.offered));
    ledger.total("prefix_reused", total(reuse.reused));
    for l in &lines {
        println!("{l}");
    }
    // The shadow's verdict is its ratchet's: every run's against the
    // baseline, a sampled one's on the roots it ran.
    let sampled = filter.is_some() || every > 1;
    let shadow_held = if sampled {
        crate::ratchet::enforce_sample(&ledger, &graded_roots)
    } else {
        crate::ratchet::enforce(ledger).map(|_| ())
    };
    if differ > 0 {
        anyhow::bail!("{differ} root(s): a warm Db disagrees with a fresh one after an edit — a memo missed an input");
    }
    shadow_held
}

/// One root: loaded warm, each edit made and undone, every step compared
/// with a fresh `Db`. The differences found, one line each, and the edits
/// that had somewhere to land.
fn grade_root(warm: &mut Db, settings: &kg_config::Settings, p: &Program, library: &Path) -> Graded {
    let mut found = Vec::new();
    let mut made = Vec::new();
    let mut counts = EditorCounts::default();
    let mut shadow = Vec::new();
    let mut kept = Vec::new();
    let mut reused = Vec::new();
    let Ok(root_path) = camino::Utf8PathBuf::from_path_buf(p.cwd.join(&p.file_arg)) else {
        return Graded { found: vec!["a path that is not UTF-8".to_owned()], ..Graded::default() };
    };
    let Ok(root_text) = std::fs::read_to_string(&root_path) else {
        return Graded { found: vec!["unreadable".to_owned()], ..Graded::default() };
    };
    let fresh = |overlays: &[(&camino::Utf8Path, &str)]| -> State {
        let mut db = Db::new(settings.clone());
        for (path, text) in overlays {
            db.set_file_text(path, (*text).to_owned());
        }
        let root = db.file_id(&root_path);
        state(&db, root)
    };
    // The editor's answers first, as the server asks for them after an
    // edit: asked after `state`, the last would be memoized already and
    // the first never taken.
    let root = warm.set_file_text(&root_path, root_text.clone());
    let mut step = |name: &str, warm: &Db, fresh: &State, found: &mut Vec<String>| {
        let answers = editor(warm, root);
        compare_editor(name, &answers, fresh, found, &mut counts);
        compare(name, &state(warm, root), fresh, found);
        if let Some(report) = warm.take_shadow_report(&[root]) {
            let mut report = report;
            for m in &mut report.mismatches {
                m.name = spell_files(&m.name, warm);
            }
            shadow.push((name.to_owned(), report));
        }
        kept.extend(warm.snapshot_size(&[root]));
        if let Some(mut report) = warm.take_reuse_report(&[root]) {
            for u in &mut report.unequal {
                *u = spell_files(u, warm);
            }
            reused.push((name.to_owned(), report));
        }
    };
    let original = fresh(&[(&root_path, &root_text)]);
    step("loaded", warm, &original, &mut found);

    // Where `LibraryBody` lands: the first library module the root
    // imports by name that has a body to edit.
    let library_edit = crate::edits::library_target(warm, root, library);

    for kind in Kind::ALL {
        let (path, before, after) = match kind {
            Kind::LibraryBody => {
                let Some(target) = library_edit.clone() else { continue };
                target
            }
            _ => {
                let Some(after) = crate::edits::apply(kind, &root_text) else { continue };
                (root_path.clone(), root_text.clone(), after)
            }
        };
        made.push(kind);
        warm.set_file_text(&path, after.clone());
        let edited = if path == root_path {
            fresh(&[(&root_path, &after)])
        } else {
            fresh(&[(&root_path, &root_text), (&path, &after)])
        };
        step(kind.name(), warm, &edited, &mut found);
        warm.set_file_text(&path, before);
        step(&format!("{} undone", kind.name()), warm, &original, &mut found);
    }
    Graded { found, made, editor: counts, shadow, kept, reused }
}

/// What the editor publishes for `root`'s file (`Db::diagnostics_layered`).
fn editor(db: &Db, root: FileId) -> Editor {
    let spell = |d: Vec<kg_diag::Diagnostic>| -> Vec<(u32, String)> {
        let mut out: Vec<(u32, String)> = d.iter().map(|d| (d.range.range.start().into(), spell_files(&format!("{d:?}"), db))).collect();
        // `kg check`'s order within a file: by start, stably
        // (`Db::check`'s `in_program_order`).
        out.sort_by_key(|(start, _)| *start);
        out
    };
    let mut first = None;
    let last = db.diagnostics_layered(&[root], root, |d| first = Some(spell(d)));
    Editor { first, last: spell(last) }
}

/// The editor's row: its last answer `kg check`'s, its first within it.
fn compare_editor(step: &str, editor: &Editor, fresh: &State, found: &mut Vec<String>, counts: &mut EditorCounts) {
    counts.steps += 1;
    if let Some(first) = &editor.first {
        counts.firsts += 1;
        let mut last = editor.last.clone();
        for d in first {
            match last.iter().position(|l| l == d) {
                Some(i) => {
                    last.remove(i);
                }
                None => found.push(format!("{step}: the editor's first answer shows what its last does not — {}", d.1)),
            }
        }
    }
    if fresh.gated_elsewhere {
        counts.gated += 1;
        return;
    }
    counts.compared += 1;
    if editor.last != fresh.root_file {
        let text = |v: &[(u32, String)]| v.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>().join("\n");
        found.push(format!(
            "{step}: the editor's last answer is not `kg check`'s — {}",
            first_difference(&text(&editor.last), &text(&fresh.root_file))
        ));
    }
}

fn compare(step: &str, warm: &State, fresh: &State, found: &mut Vec<String>) {
    if warm.diagnostics != fresh.diagnostics {
        found.push(format!("{step}: diagnostics — {}", first_difference(&warm.diagnostics, &fresh.diagnostics)));
    }
    if warm.fingerprint != fresh.fingerprint {
        found.push(format!("{step}: loop fingerprint — {}", first_difference(&warm.fingerprint, &fresh.fingerprint)));
    }
}

fn first_difference(warm: &str, fresh: &str) -> String {
    for (n, (a, b)) in warm.lines().zip(fresh.lines()).enumerate() {
        if a != b {
            let cut = |s: &str| s.chars().take(160).collect::<String>();
            return format!("line {}: warm `{}` fresh `{}`", n + 1, cut(a), cut(b));
        }
    }
    format!("{} lines against {}", warm.lines().count(), fresh.lines().count())
}

/// What `db` answers for the program rooted at `root`.
fn state(db: &Db, root: FileId) -> State {
    let roots = [root];
    let checked = db.check(&roots);
    let root_file = checked
        .iter()
        .filter(|d| d.range.file == root)
        .map(|d| (d.range.range.start().into(), spell_files(&format!("{d:?}"), db)))
        .collect();
    let gated_elsewhere = db.parse_stage(&roots).iter().any(|d| d.is_error() && d.range.file != root);
    let diagnostics: Vec<String> = checked.iter().map(|d| format!("{d:?}")).collect();
    let looped = db.looped(&roots);
    let fingerprint = kg_sem::elaborate::fingerprint(&looped.out, db.interner());
    State {
        diagnostics: spell_files(&diagnostics.join("\n"), db),
        fingerprint: spell_files(&fingerprint, db),
        root_file,
        gated_elsewhere,
    }
}

/// Every `FileId(N)` a `Debug` rendering holds, as the file's path.
fn spell_files(text: &str, db: &Db) -> String {
    spell(text, "FileId(", |n| db.path(FileId(u32::try_from(n).unwrap_or(u32::MAX))).map(|p| p.to_string()))
}

/// Every `Symbol(N)`, as its text.
pub(crate) fn spell_symbols(text: &str, db: &Db) -> String {
    spell(text, "Symbol(", |n| db.interner().resolve_index(n).map(|s| format!("{s:?}")))
}

fn spell(text: &str, marker: &str, name: impl Fn(usize) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        out.push_str(&rest[..at]);
        let after = &rest[at + marker.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match (after[..digits].parse::<usize>().ok().and_then(&name), after[digits..].starts_with(')')) {
            (Some(spelled), true) => {
                out.push_str(&spelled);
                rest = &after[digits + 1..];
            }
            _ => {
                out.push_str(marker);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
