//! Where a round's time falls, per pass and per module (sprint W7.5a) —
//! trace only, behind `KG_LOOP_PROFILE=1`, and read by nothing the loop
//! decides.
//!
//! Every per-module step of a round — each pass's visit to one module —
//! records the time it took and the module's arena length when it began.
//! A visit is **idle** when the module's arena is exactly as long as when
//! the same pass last visited it: nothing was appended in between. That is
//! the question W7.5a asks — how much of a round's floor is spent walking
//! modules that gained nothing since the pass last saw them — and an upper
//! bound on it, since inference rewrites nodes in place without appending
//! (`crate::infer`, the journal) and an idle visit can still meet new
//! types.
//!
//! [`record`] takes the same clock programmatically, with each visit's
//! round, and shows its observer every round's census as the loop leaves
//! it (sprint W7.5d step 5d-0, `cargo xtask loop-rounds`): what an edit
//! changes in each module's rounds, and what those rounds cost. Neither the
//! clock nor the observer writes anything the loop reads.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use kg_base::{ExpansionInterner, Interner};

use crate::census::Census;
use crate::infer::Module;

/// One pass's visit to one module.
#[derive(Clone, Debug)]
struct Visit {
    pass: &'static str,
    module: usize,
    len: usize,
    round: Option<u32>,
    nanos: u64,
}

thread_local! {
    static PROFILE: RefCell<Option<Profile>> = const { RefCell::new(None) };
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static OBSERVER: RefCell<Option<Box<Observer>>> = const { RefCell::new(None) };
    static FINISHED: RefCell<Vec<Recording>> = const { RefCell::new(Vec::new()) };
}

/// What [`record`]'s observer is shown after each round's census.
type Observer = dyn FnMut(&Seen<'_>);

/// Visits counted inside a whole-program step's own time, so left out of
/// the summary's total: inference's walk of each module is inside
/// `inferType`.
const INSIDE_WHOLE: [&str; 1] = ["inferType walk"];

#[derive(Default)]
struct Profile {
    round: u32,
    /// The round under way: `None` before the loop's first (passes 1–7).
    iteration: Option<u32>,
    visits: Vec<Visit>,
    /// A step timed as a whole, not per module: `(pass, round, nanos)`.
    whole: Vec<(&'static str, Option<u32>, u64)>,
}

/// Whether `KG_LOOP_PROFILE` is set.
pub fn enabled() -> bool {
    crate::env_flag!("KG_LOOP_PROFILE")
}

/// Start recording on this thread (a no-op unless [`enabled`] or inside
/// [`record`]).
pub(crate) fn begin() {
    if enabled() || RECORDING.with(Cell::get) {
        PROFILE.with(|p| *p.borrow_mut() = Some(Profile::default()));
    }
}

pub(crate) fn set_round(round: u32) {
    PROFILE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.round = round;
            p.iteration = Some(round);
        }
    });
}

/// A visit's clock, recorded when dropped.
#[must_use]
pub(crate) struct VisitGuard {
    pass: &'static str,
    module: usize,
    len: usize,
    start: Option<Instant>,
}

/// Time `pass`'s visit to `module`, whose arena holds `len` nodes now.
pub(crate) fn visit(pass: &'static str, module: usize, len: usize) -> VisitGuard {
    let on = PROFILE.with(|p| p.borrow().is_some());
    VisitGuard { pass, module, len, start: on.then(Instant::now) }
}

impl Drop for VisitGuard {
    fn drop(&mut self) {
        let Some(start) = self.start else { return };
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        PROFILE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                let round = p.iteration;
                p.visits.push(Visit { pass: self.pass, module: self.module, len: self.len, round, nanos });
            }
        });
    }
}

/// A step timed whole — one that is not a walk per module (a symbol map
/// built over every module, the census's union).
pub(crate) fn whole(pass: &'static str, start: Instant) {
    let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
    PROFILE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            let round = p.iteration;
            p.whole.push((pass, round, nanos));
        }
    });
}

/// Stop recording; print the summary to stderr when [`enabled`] — per
/// pass, its time in all, in idle visits, and in the root (`root`, the
/// last module) against the rest — and hand it to [`record`] inside one.
pub(crate) fn end(modules: usize) {
    let Some(p) = PROFILE.with(|p| p.borrow_mut().take()) else { return };
    if RECORDING.with(Cell::get) {
        // Inference's walk of each module is attributed to the module, and
        // only the rest of `inferType` — its symbol map, its settling — is
        // left whole. A round runs inference more than once, so a round's
        // walks are taken from its `inferType` steps in turn.
        let mut walks: std::collections::BTreeMap<Option<u32>, u64> = std::collections::BTreeMap::new();
        let mut timed: Vec<Timed> = Vec::with_capacity(p.visits.len() + p.whole.len());
        for v in &p.visits {
            if INSIDE_WHOLE.contains(&v.pass) {
                *walks.entry(v.round).or_default() += v.nanos;
            }
            timed.push(Timed { pass: v.pass, module: Some(v.module), round: v.round, nanos: v.nanos });
        }
        for &(pass, round, nanos) in &p.whole {
            let inside = match walks.get_mut(&round) {
                Some(w) if pass == "inferType" => {
                    let taken = (*w).min(nanos);
                    *w -= taken;
                    taken
                }
                _ => 0,
            };
            timed.push(Timed { pass, module: None, round, nanos: nanos - inside });
        }
        FINISHED.with(|f| f.borrow_mut().push(Recording { modules, rounds: p.round, timed }));
    }
    if !enabled() {
        return;
    }
    let mut last: std::collections::HashMap<(&'static str, usize), usize> = std::collections::HashMap::new();
    #[derive(Default)]
    struct Row {
        total: u64,
        idle: u64,
        root: u64,
        visits: u64,
        idle_visits: u64,
    }
    let mut rows: std::collections::BTreeMap<&'static str, Row> = std::collections::BTreeMap::new();
    let root = modules.saturating_sub(1);
    for v in &p.visits {
        let idle = last.insert((v.pass, v.module), v.len) == Some(v.len);
        let r = rows.entry(v.pass).or_default();
        r.total += v.nanos;
        r.visits += 1;
        if idle {
            r.idle += v.nanos;
            r.idle_visits += 1;
        }
        if v.module == root {
            r.root += v.nanos;
        }
    }
    let mut whole: std::collections::BTreeMap<&'static str, u64> = std::collections::BTreeMap::new();
    for (pass, _, n) in &p.whole {
        *whole.entry(pass).or_default() += n;
    }
    let s = |n: u64| n as f64 / 1e9;
    let (mut all, mut idle) = (0u64, 0u64);
    eprintln!("loop profile: {modules} modules, {} rounds; per pass: all / idle visits (nothing appended since the pass last ran) / the root", p.round);
    for (pass, r) in &rows {
        let inside = INSIDE_WHOLE.contains(pass);
        if !inside {
            all += r.total;
            idle += r.idle;
        }
        eprintln!(
            "  {pass:<22} {:>7.3}s  idle {:>7.3}s ({}/{} visits)  root {:>7.3}s{}",
            s(r.total),
            s(r.idle),
            r.idle_visits,
            r.visits,
            s(r.root),
            if inside { "  (inside its whole step)" } else { "" }
        );
    }
    for (pass, n) in &whole {
        all += n;
        eprintln!("  {pass:<22} {:>7.3}s  (whole program)", s(*n));
    }
    eprintln!("  {:<22} {:>7.3}s  idle {:>7.3}s ({:.0}%)", "in all", s(all), s(idle), 100.0 * idle as f64 / all.max(1) as f64);
    print_rounds(&p, root);
}

/// The same time by round, the passes grouped: where a run's rounds spend
/// it, and how it tails off towards the fixpoint. Inference is its whole
/// step, the walks inside it; `symbols` the maps the census, deduction and
/// the wrappers build; `root` the root module's visits, its walks included.
fn print_rounds(p: &Profile, root: usize) {
    const COLUMNS: [&str; 9] = ["prefix", "census", "instant.", "deduce", "wrappers", "closures", "infer", "symbols", "other"];
    let column = |pass: &str| match pass {
        "prefix" => 0,
        "census" => 1,
        "instantiate" => 2,
        "deduceTemplateArgs" => 3,
        "addLambdaWrappers" => 4,
        "closureCalls" => 5,
        "inferType" => 6,
        _ if pass.ends_with(" symbols") => 7,
        _ => 8,
    };
    let mut rounds: std::collections::BTreeMap<Option<u32>, ([u64; 9], u64)> = std::collections::BTreeMap::new();
    for v in &p.visits {
        let (cells, in_root) = rounds.entry(v.round).or_default();
        if !INSIDE_WHOLE.contains(&v.pass) {
            cells[column(v.pass)] += v.nanos;
        }
        if v.module == root {
            *in_root += v.nanos;
        }
    }
    for &(pass, round, n) in &p.whole {
        rounds.entry(round).or_default().0[column(pass)] += n;
    }
    let ms = |n: u64| n as f64 / 1e6;
    eprint!("per round, ms:  {:>6} {:>8}", "round", "all");
    for c in COLUMNS {
        eprint!(" {c:>8}");
    }
    eprintln!(" {:>8}", "root");
    for (round, (cells, in_root)) in &rounds {
        let label = round.map_or_else(|| "before".to_owned(), |r| r.to_string());
        eprint!("                {label:>6} {:>8.1}", ms(cells.iter().sum()));
        for &n in cells {
            eprint!(" {:>8.1}", ms(n));
        }
        eprintln!(" {:>8.1}", ms(*in_root));
    }
}

/// One round's census as the loop leaves it, shown to [`record`]'s
/// observer before anything is instantiated.
#[derive(Debug)]
pub struct Seen<'a> {
    /// The round whose census this is: 0 before the first instantiation.
    pub round: u32,
    /// The program in that round, in program order.
    pub modules: &'a [Module],
    pub census: &'a Census,
    pub interner: &'a Interner,
    pub expansions: &'a ExpansionInterner,
}

/// The time one step took: a pass's visit to one module, or a step over
/// the whole program (`module` `None`) — `inferType` less its walks of
/// each module, which are the `inferType walk` visits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    pub pass: &'static str,
    pub module: Option<usize>,
    /// `None` before the loop's first round (passes 1–7).
    pub round: Option<u32>,
    pub nanos: u64,
}

/// One loop's clock, as [`record`] hands it back.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub modules: usize,
    pub rounds: u32,
    pub timed: Vec<Timed>,
}

/// Run `f` with the loop's clock on and `observer` shown every round's
/// census ([`Seen`]); `f`'s value and one [`Recording`] per loop it ran on
/// this thread. Behaviour is unchanged: the clock and the observer only
/// read.
pub fn record<R>(observer: impl FnMut(&Seen<'_>) + 'static, f: impl FnOnce() -> R) -> (R, Vec<Recording>) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RECORDING.with(|r| r.set(false));
            OBSERVER.with(|o| o.borrow_mut().take());
            PROFILE.with(|p| p.borrow_mut().take());
        }
    }
    RECORDING.with(|r| r.set(true));
    OBSERVER.with(|o| *o.borrow_mut() = Some(Box::new(observer)));
    FINISHED.with(|f| f.borrow_mut().clear());
    let reset = Reset;
    let out = f();
    drop(reset);
    (out, FINISHED.with(|f| std::mem::take(&mut *f.borrow_mut())))
}

/// Show [`record`]'s observer, if any, round `round`'s census.
pub(crate) fn census(round: u32, modules: &[Module], census: &Census, interner: &Interner, expansions: &ExpansionInterner) {
    OBSERVER.with(|o| {
        if let Some(observe) = o.borrow_mut().as_mut() {
            observe(&Seen { round, modules, census, interner, expansions });
        }
    });
}
