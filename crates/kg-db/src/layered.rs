//! The editor's layered answers (sprint W7.6, the owner's answer of
//! 2026-10-10): under [`PassBudget::Layered`](crate::PassBudget) a
//! program's loop runs on a thread of its own and answers twice — first as
//! it passes the budget's bound, a fork of its own state there
//! (`kg_sem::elaborate::elaborate_layered`), and last at the fixpoint, the
//! answer `kg check` gives.
//!
//! **Why a thread of its own.** Today's loop ran on whichever caller asked
//! first, and a second caller waited for it; here that caller would hold
//! the whole run to the fixpoint. The editor asks from three places at
//! once after an edit — the diagnostics, and inlay hints and code actions
//! on the request pool — and whichever asked first would wait the 2.3 s of
//! `risc-v.k`'s fixpoint for an answer the first bound gives in 0.9 s. So
//! the loop belongs to no caller: each waits for the answer it wants
//! ([`Want`]) and goes, and the run goes on. What the loop reads it is
//! given before it starts — the lowered modules, owned, and the interner
//! and the tokens, shared — and what it hands back is the elaboration
//! alone; the caller that first wants an answer builds its [`Looped`]
//! (`crate::looped::finish`, which reads the database), once.
//!
//! **Cancellation.** The run answers to the database's token, which the
//! server raises as it queues an edit, and to its own ([`Flight::cancel`]),
//! which every input change raises (`Db::retire_flights`) and nothing
//! lowers: the database's token is lowered as the edit is applied, and a
//! run no reader holds the database for could miss a raise that brief. A
//! cancelled run writes no memo. A first answer already built keeps its
//! memo — it finished, and is a correct answer at its bound — so a buffer
//! that stops parsing reads it as the last loop (sprint W3.4).
//!
//! **Waiting.** A caller waits on the flight's condition variable a slice
//! at a time and between slices checks the database's token — an edit
//! makes its answer stale — and its own request's (`kg_base::cancel::
//! checkpoint`). A builder is detached from its request, as `Db::looped`'s
//! run is: others may be waiting on what it builds.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use kg_base::cancel::{Cancel, Cancelled};
use kg_base::{FileId, Interner};
use kg_sem::elaborate::Elaborated;

use crate::db::Db;
use crate::looped::Looped;
use crate::program::Program;
use crate::stats::{Phase, Query, Stats};

/// Which of a layered loop's answers a caller wants.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    /// The newest ready of the two, waiting no longer than for the first.
    First,
    /// The fixpoint's.
    Last,
}

/// One program's loop under a layered budget, while it runs: at most one
/// per root set ([`crate::Db::looped`]'s memo is its home once it ends).
pub(crate) struct Flight {
    pub(crate) program: Arc<Program>,
    /// The run's own token: raised when the database's inputs change or
    /// the database is dropped, and never lowered.
    pub(crate) cancel: Cancel,
    /// What `crate::looped::finish` reads of the lowering — set by the
    /// caller that started the run, before the run starts.
    inputs: OnceLock<crate::looped::Inputs>,
    state: Mutex<Stages>,
    changed: Condvar,
}

impl std::fmt::Debug for Flight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flight").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Stages {
    first: Stage,
    last: Stage,
    /// Why the run stopped with no last answer.
    ended: Option<Ended>,
}

impl Stages {
    fn stage(&mut self, want: Want) -> &mut Stage {
        match want {
            Want::First => &mut self.first,
            Want::Last => &mut self.last,
        }
    }
}

#[derive(Default)]
enum Stage {
    #[default]
    Pending,
    /// The run has passed it; no caller has built its `Looped` yet.
    Reached(Arc<Elaborated>),
    /// A caller is building its `Looped`.
    Building,
    Built(Arc<Looped>),
    /// No first answer: the run ended by the bound, so its last is its
    /// only one.
    Never,
}

enum Ended {
    Cancelled,
    Panicked(String),
}

impl Flight {
    fn new(program: Arc<Program>) -> Flight {
        Flight {
            program,
            cancel: Cancel::new(),
            inputs: OnceLock::new(),
            state: Mutex::default(),
            changed: Condvar::new(),
        }
    }

    /// The stages, whatever a panic elsewhere left the lock: each write
    /// to them is one assignment, so a panic never leaves them half-made.
    fn stages(&self) -> std::sync::MutexGuard<'_, Stages> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ended(&self) -> bool {
        self.stages().ended.is_some()
    }

    /// Whether the run should fork its state for a first answer: no caller
    /// has one already.
    fn wants_first(&self) -> bool {
        matches!(self.stages().first, Stage::Pending)
    }

    fn reached(&self, want: Want, out: Arc<Elaborated>) {
        let mut st = self.stages();
        let stage = st.stage(want);
        if matches!(stage, Stage::Pending) {
            *stage = Stage::Reached(out);
        }
        if want == Want::Last && matches!(st.first, Stage::Pending) {
            st.first = Stage::Never;
        }
        drop(st);
        self.changed.notify_all();
    }

    fn end(&self, why: Ended) {
        self.stages().ended.get_or_insert(why);
        self.changed.notify_all();
    }
}

/// Ends a flight whose starter or builder unwinds — the database's token,
/// an edit on the way: a waiter would otherwise wait on a stage no one
/// will finish.
struct Abandon<'a>(Option<&'a Flight>);

impl Abandon<'_> {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for Abandon<'_> {
    fn drop(&mut self) {
        if let Some(flight) = self.0 {
            flight.end(Ended::Cancelled);
        }
    }
}

/// `want` of the loop over `roots`' program under a layered budget whose
/// first answer is taken at round `at` — or the last where it is ready —
/// with whether it is the last.
pub(crate) fn looped(db: &Db, roots: &[FileId], at: u32, want: Want) -> (Arc<Looped>, bool) {
    let program = db.program(roots);
    let first = match db.looped_memo(roots, &program) {
        Some((value, true)) => {
            db.stats.hit(Query::Looped);
            return (value, true);
        }
        Some((value, false)) if want == Want::First => {
            db.stats.hit(Query::Looped);
            return (value, false);
        }
        Some((value, false)) => Some(value),
        None => None,
    };
    let flight = flight(db, roots, &program, at, first);
    wait(db, roots, &flight, want)
}

/// The flight running for `program`, started if none is.
fn flight(db: &Db, roots: &[FileId], program: &Arc<Program>, at: u32, first: Option<Arc<Looped>>) -> Arc<Flight> {
    let key = roots.to_vec();
    let flight = {
        let mut flights = db.flights().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(f) = flights.get(&key)
            && Arc::ptr_eq(&f.program, program)
            && !f.ended()
        {
            db.stats.hit(Query::Looped);
            return f.clone();
        }
        let f = Arc::new(Flight::new(program.clone()));
        if let Some(first) = first {
            // A first answer already built — the run before this one,
            // stopped after it: this run needs no fork.
            f.stages().first = Stage::Built(first);
        }
        flights.insert(key, f.clone());
        f
    };
    start(db, roots, &flight, at);
    flight
}

/// Lowers `roots`' program for `flight` and starts its run.
fn start(db: &Db, roots: &[FileId], flight: &Arc<Flight>, at: u32) {
    let abandon = Abandon(Some(flight));
    let _miss = db.stats.missed(Query::Looped);
    let token = db.cancel_token();
    let crate::looped::Lowered { inputs, irs } =
        kg_base::cancel::detached(|| kg_base::cancel::scope(&token, || crate::looped::lowered(db, roots)));
    // The shadow of the run this one follows, where the database checks
    // its kept snapshots (`crate::snapshot`).
    let keeper = Arc::clone(db.keeper());
    let shadow = keeper.shadowing().then(|| keeper.kept(roots)).flatten().and_then(|k| k.shadow(&inputs));
    // And what it reuses of that run (`kg_sem::reuse`, D1 step 3).
    let reuse = keeper.reusing().and_then(|check| keeper.kept(roots).and_then(|k| k.reuse(&inputs, check)));
    let keep = Keep { keeper, roots: roots.to_vec(), inputs: inputs.clone(), shadow, reuse };
    let _ = flight.inputs.set(inputs);
    let interner = Arc::clone(db.shared_interner());
    let full = db.settings().verdicts.infer_full;
    let stats = Arc::clone(&db.stats);
    let f = Arc::clone(flight);
    std::thread::Builder::new()
        .name("kg-loop".to_owned())
        .spawn(move || run(&f, irs, &interner, at, full, &token, &stats, keep))
        .expect("spawning the loop's thread");
    abandon.disarm();
}

/// What a run keeps when it reaches its fixpoint (`crate::snapshot`): its
/// snapshot, with what it was run on, and the shadow's report.
struct Keep {
    keeper: Arc<crate::snapshot::Keeper>,
    roots: Vec<FileId>,
    inputs: crate::looped::Inputs,
    shadow: Option<kg_sem::snapshot::Shadow>,
    reuse: Option<kg_sem::reuse::Reuse>,
}

/// The run: to the fixpoint, a first answer offered at round `at`, its
/// state recorded per module and round and kept with the last.
#[allow(clippy::too_many_arguments)]
fn run(flight: &Flight, irs: Vec<kg_hir::ModuleIr>, interner: &Interner, at: u32, full: bool, database: &Cancel, stats: &Stats, keep: Keep) {
    let Keep { keeper, roots, inputs, mut shadow, reuse } = keep;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Inside both tokens' scopes, so a walk with no token of its own —
        // `desugar`'s — stops too (`kg_base::cancel::checkpoint_strided`).
        kg_base::cancel::scope(database, || {
            kg_base::cancel::scope(&flight.cancel, || {
                let _t = stats.phase(Phase::Loop);
                let rounds = kg_sem::elaborate::ROUND_GUARD;
                let names = inputs.modules.iter().map(|m| format!("{m:?}")).collect();
                let mut observe = |e: &kg_sem::snapshot::Ended<'_>| {
                    if let Some(s) = shadow.as_mut() {
                        s.observe(e);
                    }
                };
                let record = kg_sem::snapshot::Recorder::observed(names, &mut observe).reusing(reuse);
                let mut first = |stopped: kg_sem::elaborate::Stopped<'_>| {
                    if flight.wants_first() {
                        let answer = {
                            let _t = stats.phase(Phase::FirstAnswer);
                            stopped.answer()
                        };
                        flight.reached(Want::First, Arc::new(answer));
                    }
                };
                kg_sem::elaborate::elaborate_layered_recorded(irs, interner, rounds, at, full, &flight.cancel, &mut first, record)
            })
        })
    }));
    match outcome {
        Ok(out) => {
            let out = Arc::new(out);
            if let Some(s) = shadow {
                keeper.report(roots.clone(), s.finish());
            }
            if let Some(r) = out.snapshot.as_ref().and_then(|s| s.reused.clone()) {
                keeper.report_reuse(roots.clone(), r);
            }
            let gone = keeper.keep(roots, crate::snapshot::Kept { out: Arc::clone(&out), inputs });
            flight.reached(Want::Last, out);
            // The run before's program, freed once the answer is out.
            drop(gone);
        }
        Err(payload) if payload.is::<Cancelled>() => flight.end(Ended::Cancelled),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            flight.end(Ended::Panicked(message));
        }
    }
}

/// How long a waiter sleeps between looks at the tokens: well under the
/// 50 ms a cancellation is allowed (sprint W7.2's exit).
const SLICE: Duration = Duration::from_millis(10);

fn wait(db: &Db, roots: &[FileId], flight: &Arc<Flight>, want: Want) -> (Arc<Looped>, bool) {
    // What answers `want`, newest first: a built answer before one still
    // to build, so a hover takes the first answer at once rather than
    // building the last one the diagnostics are about to.
    let answers: &[Want] = match want {
        Want::First => &[Want::Last, Want::First],
        Want::Last => &[Want::Last],
    };
    loop {
        let mut st = flight.stages();
        for &w in answers {
            if let Stage::Built(looped) = st.stage(w) {
                return (looped.clone(), w == Want::Last);
            }
        }
        for &w in answers {
            let stage = st.stage(w);
            if matches!(stage, Stage::Reached(_)) {
                let Stage::Reached(out) = std::mem::replace(stage, Stage::Building) else { unreachable!("matched") };
                drop(st);
                let looped = build(db, roots, flight, w, out);
                return (looped, w == Want::Last);
            }
        }
        // Unwound out of with the lock released: a guard dropped while
        // unwinding poisons it for every other waiter.
        let ended = st.ended.as_ref().map(|e| match e {
            Ended::Cancelled => None,
            Ended::Panicked(message) => Some(message.clone()),
        });
        if let Some(ended) = ended {
            drop(st);
            match ended {
                None => std::panic::resume_unwind(Box::new(Cancelled)),
                Some(message) => panic!("the program-wide loop panicked: {message}"),
            }
        }
        drop(flight.changed.wait_timeout(st, SLICE).unwrap_or_else(std::sync::PoisonError::into_inner));
        db.cancel_token().check();
        kg_base::cancel::checkpoint();
    }
}

/// Builds `want`'s [`Looped`] from the run's elaboration and hands it to
/// every waiter, then to the memo: the last as `Db::looped`'s answer, the
/// first as the newest loop where no last is there.
fn build(db: &Db, roots: &[FileId], flight: &Arc<Flight>, want: Want, out: Arc<Elaborated>) -> Arc<Looped> {
    let abandon = Abandon(Some(flight));
    let inputs = flight.inputs.get().cloned().expect("set before the run started");
    let token = db.cancel_token();
    let looped = Arc::new(kg_base::cancel::detached(|| {
        kg_base::cancel::scope(&token, || {
            let _t = db.stats.phase(Phase::Query(Query::Looped));
            crate::looped::finish(db, inputs, out, want == Want::First, want == Want::Last)
        })
    }));
    abandon.disarm();
    *flight.stages().stage(want) = Stage::Built(looped.clone());
    flight.changed.notify_all();
    db.remember_looped(roots, &flight.program, &looped, want == Want::Last);
    if want == Want::Last {
        let mut flights = db.flights().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if flights.get(roots).is_some_and(|f| Arc::ptr_eq(f, flight)) {
            flights.remove(roots);
        }
    }
    looped
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kg_base::Utf8PathBuf;

    use crate::stats::Query;
    use crate::{Db, PassBudget};

    /// `Down<n>` calls `Down<n - 1>` down to `Down<0>`, whose `bool[0]` is
    /// the one error — reached `n + 1` rounds in, past the editor's first
    /// bound of four — each copy carrying `body` statements, so the run past
    /// the bound takes long enough to be caught in.
    pub(crate) fn down(n: usize, body: usize) -> String {
        let body: String = (0..body).map(|i| format!("    const auto v{i} = cast<uint32>(a + {i}) * b;\n")).collect();
        format!(
            "template <auto N>\ninline uint32 Down(uint32 a, uint32 b)\n{{\n    bool[N] x;\n{body}    static if (N > 0)\n    {{\n        return Down<N - 1>(a, b);\n    }}\n    else\n    {{\n        return a;\n    }}\n}}\n\ninline uint32 main(uint32 a)\n{{\n    return Down<{n}>(a, 3);\n}}\n"
        )
    }

    fn db(budget: PassBudget, text: &str) -> (Db, kg_base::FileId) {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = Db::new(settings);
        db.set_pass_budget(budget);
        let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
        (db, main)
    }

    fn messages(d: &[kg_diag::Diagnostic]) -> Vec<String> {
        d.iter().map(|d| format!("{} {:?} {}", d.message, d.range, d.labels.len())).collect()
    }

    /// The editor's diagnostics answer twice from one run: the first at
    /// four rounds, which a truncated loop leaves without `Down<0>`'s error,
    /// and the last at the fixpoint, a batch compile's own — with the loop
    /// run once (sprint W7.6). Red with the last answer started afresh
    /// (two runs), the first answer skipped, or the last read off the first.
    #[test]
    fn the_editor_answers_first_at_its_bound_and_last_at_the_fixpoint_from_one_run() {
        let text = down(40, 50);
        let (editor, main) = db(PassBudget::EDITOR, &text);
        let mut first = None;
        let last = editor.diagnostics_layered(&[main], main, |d| first = Some(d));
        let first = first.expect("a first answer: the loop runs past four rounds");
        assert_eq!(messages(&first), Vec::<String>::new());
        let (batch, batch_main) = db(PassBudget::Fixpoint, &text);
        let check = batch.check(&[batch_main]);
        assert_eq!(check.len(), 1, "{check:?}");
        assert_eq!(messages(&last), messages(&check));
        assert_eq!(editor.stats().misses(Query::Looped), 1, "the last answer is the same run's");
        assert_eq!(editor.looped(&[main]).out.report.rounds, 41);
        assert!(editor.budget_exhausted(&[main]).is_none());
    }

    /// An edit while the fixpoint runs on stops it: the database's token
    /// unwinds the reader waiting for the last answer, the run stops, and
    /// no last answer is memoized — the first stays the newest loop, as an
    /// editor shows the first answer until the next edit's replaces it.
    /// The next ask runs the program again, and answers as a fresh
    /// database does. Red with the run deaf to the tokens (the last answer
    /// lands), or a cancelled run's last answer kept.
    #[test]
    fn an_edit_while_the_fixpoint_runs_stops_it_and_keeps_no_last_answer() {
        let text = down(120, 300);
        let (mut editor, main) = db(PassBudget::EDITOR, &text);
        let (first, last) = editor.looped_first(&[main]);
        assert!(!last, "the run must still be short of its fixpoint, or the test grades nothing");
        let flight = editor.flights().lock().unwrap().get(&vec![main]).cloned();
        let flight = flight.expect("the run goes on");
        // The server raises the token as it queues an edit.
        editor.cancel_token().raise();
        let waited = kg_base::cancel::catch(|| editor.looped(&[main]));
        assert!(waited.is_err(), "the reader of the last answer is cancelled");
        stops(&flight);
        let ready = editor.editor_loop_ready(&[main]).expect("the first answer is kept");
        assert!(Arc::ptr_eq(ready.looped(), &first), "the newest loop is the first answer");
        assert!(editor.budget_exhausted(&[main]).is_none());

        let edited = down(3, 1);
        editor.set_file_text(&Utf8PathBuf::from("/proj/main.k"), edited.clone());
        let mut again = None;
        let answer = editor.diagnostics_layered(&[main], main, |d| again = Some(d));
        assert!(again.is_none(), "`Down<3>` ends by the bound: one answer");
        let (fresh, fresh_main) = db(PassBudget::Fixpoint, &edited);
        assert_eq!(messages(&answer), messages(&fresh.check(&[fresh_main])));
    }

    /// Waits for `flight`'s run to stop short of its fixpoint — red if it
    /// reaches it instead.
    fn stops(flight: &super::Flight) {
        let started = std::time::Instant::now();
        while !flight.ended() {
            let short = matches!(flight.stages().last, super::Stage::Pending);
            assert!(short, "the run went on to its fixpoint");
            assert!(started.elapsed().as_secs() < 60, "the run did not stop");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// An input change stops the run whether or not anyone holds the
    /// database: its own token, which the change raises and nothing
    /// lowers — the database's is lowered by the very change, and a run no
    /// reader waits on could miss a raise that brief. Red with
    /// `Db::retire_flights` raising nothing.
    #[test]
    fn an_input_change_stops_a_run_no_one_waits_on() {
        let text = down(120, 300);
        let (mut editor, main) = db(PassBudget::EDITOR, &text);
        let (_, last) = editor.looped_first(&[main]);
        assert!(!last, "the run must still be short of its fixpoint, or the test grades nothing");
        let flight = editor.flights().lock().unwrap().get(&vec![main]).cloned();
        let flight = flight.expect("the run goes on");
        editor.set_file_text(&Utf8PathBuf::from("/proj/other.k"), "inline void f() {}\n".to_owned());
        assert!(!editor.cancel_token().is_raised());
        stops(&flight);
    }

    /// While the fixpoint runs on, a reader of the loop is answered from
    /// the first answer at once — no wait for the last, no second run —
    /// and once the last is there, from it. Red with `Db::editor_loop`
    /// reading the last answer (it waits), or the flight not shared.
    #[test]
    fn a_reader_while_the_fixpoint_runs_reads_the_first_answer() {
        let text = down(120, 300);
        let (editor, main) = db(PassBudget::EDITOR, &text);
        let read = editor.editor_loop(&[main], main).expect("the buffer parses");
        let flight = editor.flights().lock().unwrap().get(&vec![main]).cloned();
        let flight = flight.expect("the run goes on");
        assert!(
            matches!(flight.stages().last, super::Stage::Pending),
            "answered before the fixpoint was reached"
        );
        assert!(read.looped().out.report.budget_exhausted && read.looped().out.report.rounds == 4);
        let again = editor.editor_loop(&[main], main).expect("parses");
        assert!(Arc::ptr_eq(again.looped(), read.looped()));
        let last = editor.looped(&[main]);
        assert_eq!(last.out.report.rounds, 121);
        assert_eq!(editor.stats().misses(Query::Looped), 1, "one run for every reader");
        let after = editor.editor_loop(&[main], main).expect("parses");
        assert!(Arc::ptr_eq(after.looped(), &last), "the last answer, once it is there");
    }
}
