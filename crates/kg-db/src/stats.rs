//! What a query costs: each memo's hits and misses, and the time spent in
//! each phase, exclusive of the phases nested inside it (sprint W0.5,
//! W7.1).
//!
//! The counters are the instrument the query layer's mechanisms are graded
//! by before any of them is built: "a body edit misses `item_tree` once and
//! `program` never" is a unit test over [`Stats`], not a timing
//! (`docs/plans/sprint-parity.md` W7.1). They are always on: an atomic add
//! per query and two clock reads per miss, against a miss that parses,
//! lowers or elaborates.
//!
//! **Exclusive time.** A phase's clock runs from its guard's creation to its
//! drop, less the time of every phase that ran inside it on the same
//! thread, so the rows of one keystroke add up to its wall time rather than
//! to a multiple of it: `program` does not also count the item trees it
//! asked for. Nesting is per thread; a gate's threads each keep their own
//! stack and share the totals.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A memoized query, or an unmemoized one whose calls are counted as
/// misses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Query {
    Parse,
    ItemTree,
    Program,
    Symbols,
    Looped,
    /// `sorted::sorted_program` — the program's assembly, unmemoized: each
    /// module's memo looked up and its key checked.
    Sorted,
    /// One module's sorted lowering, memoized by what it read (W7.3,
    /// `sorted::SortedMemo`) — hits and misses counted per module.
    SortedModule,
    /// `Db::frontend_diagnostics`, one module's CST passes — memoized
    /// nowhere, by design (`db.rs`, "What is deliberately not memoized").
    CstPasses,
}

impl Query {
    pub const ALL: [Query; 8] = [
        Query::Parse,
        Query::ItemTree,
        Query::Program,
        Query::Symbols,
        Query::Looped,
        Query::Sorted,
        Query::SortedModule,
        Query::CstPasses,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Query::Parse => "parse",
            Query::ItemTree => "item_tree",
            Query::Program => "program",
            Query::Symbols => "global_symbols",
            Query::Looped => "looped",
            Query::Sorted => "sorted lowering",
            Query::SortedModule => "sorted module",
            Query::CstPasses => "CST passes",
        }
    }
}

/// Where time goes: the memoized queries' misses, and the loop's stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// A query's own work on a miss, nested phases excluded.
    Query(Query),
    /// `kg_sem::elaborate` — passes 2–8, the rounds.
    Loop,
    /// `kg_sem::check` over the loop's copies.
    CopiesCheck,
    /// The editor's first answer: the loop's state forked where its bound
    /// would have stopped it (`kg_sem::elaborate::Stopped::answer`, sprint
    /// W7.6).
    FirstAnswer,
}

impl Phase {
    pub const ALL: [Phase; 11] = [
        Phase::Query(Query::Parse),
        Phase::Query(Query::ItemTree),
        Phase::Query(Query::Program),
        Phase::Query(Query::Symbols),
        Phase::Query(Query::Looped),
        Phase::Query(Query::Sorted),
        Phase::Query(Query::SortedModule),
        Phase::Query(Query::CstPasses),
        Phase::Loop,
        Phase::CopiesCheck,
        Phase::FirstAnswer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Phase::Query(q) => q.name(),
            Phase::Loop => "the loop",
            Phase::CopiesCheck => "copies' check",
            Phase::FirstAnswer => "first answer's fork",
        }
    }

    fn index(self) -> usize {
        Phase::ALL.iter().position(|&p| p == self).expect("every phase is listed")
    }
}

impl Query {
    fn index(self) -> usize {
        Query::ALL.iter().position(|&q| q == self).expect("every query is listed")
    }
}

/// The counters a [`crate::Db`] keeps.
pub struct Stats {
    hits: [AtomicU64; Query::ALL.len()],
    misses: [AtomicU64; Query::ALL.len()],
    nanos: [AtomicU64; Phase::ALL.len()],
    entered: [AtomicU64; Phase::ALL.len()],
}

impl std::fmt::Debug for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stats { .. }")
    }
}

impl Default for Stats {
    fn default() -> Stats {
        Stats {
            hits: std::array::from_fn(|_| AtomicU64::new(0)),
            misses: std::array::from_fn(|_| AtomicU64::new(0)),
            nanos: std::array::from_fn(|_| AtomicU64::new(0)),
            entered: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

thread_local! {
    /// The phases open on this thread: each one's start, and the time its
    /// nested phases have taken so far.
    static OPEN: RefCell<Vec<(Instant, u64)>> = const { RefCell::new(Vec::new()) };
}

/// A phase's clock, stopped when dropped.
#[derive(Debug)]
#[must_use = "the phase is timed until the guard drops"]
pub struct PhaseGuard<'a> {
    stats: &'a Stats,
    phase: Phase,
}

impl Drop for PhaseGuard<'_> {
    fn drop(&mut self) {
        let Some((start, nested)) = OPEN.with(|o| o.borrow_mut().pop()) else { return };
        let total = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.stats.nanos[self.phase.index()].fetch_add(total.saturating_sub(nested), Ordering::Relaxed);
        OPEN.with(|o| {
            if let Some(parent) = o.borrow_mut().last_mut() {
                parent.1 = parent.1.saturating_add(total);
            }
        });
    }
}

impl Stats {
    pub(crate) fn hit(&self, q: Query) {
        self.hits[q.index()].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn miss(&self, q: Query) {
        self.misses[q.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// A query missed: counted, and its work timed until the guard drops.
    pub(crate) fn missed(&self, q: Query) -> PhaseGuard<'_> {
        self.miss(q);
        self.phase(Phase::Query(q))
    }

    /// Time `phase` until the guard drops.
    pub(crate) fn phase(&self, phase: Phase) -> PhaseGuard<'_> {
        self.entered[phase.index()].fetch_add(1, Ordering::Relaxed);
        OPEN.with(|o| o.borrow_mut().push((Instant::now(), 0)));
        PhaseGuard { stats: self, phase }
    }

    /// Every counter as it stands.
    pub fn snapshot(&self) -> Snapshot {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        Snapshot {
            hits: self.hits.each_ref().map(load),
            misses: self.misses.each_ref().map(load),
            nanos: self.nanos.each_ref().map(load),
            entered: self.entered.each_ref().map(load),
        }
    }
}

/// The counters at one moment; subtract two for what happened between.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    hits: [u64; Query::ALL.len()],
    misses: [u64; Query::ALL.len()],
    nanos: [u64; Phase::ALL.len()],
    entered: [u64; Phase::ALL.len()],
}

impl Snapshot {
    pub fn hits(&self, q: Query) -> u64 {
        self.hits[q.index()]
    }

    pub fn misses(&self, q: Query) -> u64 {
        self.misses[q.index()]
    }

    /// Exclusive time in `phase`, in seconds.
    pub fn seconds(&self, phase: Phase) -> f64 {
        self.nanos[phase.index()] as f64 / 1e9
    }

    /// What happened after `earlier`.
    pub fn since(&self, earlier: &Snapshot) -> Snapshot {
        let sub = |a: &[u64], b: &[u64], out: &mut [u64]| {
            for ((o, x), y) in out.iter_mut().zip(a).zip(b) {
                *o = x.saturating_sub(*y);
            }
        };
        let mut d = Snapshot::default();
        sub(&self.hits, &earlier.hits, &mut d.hits);
        sub(&self.misses, &earlier.misses, &mut d.misses);
        sub(&self.nanos, &earlier.nanos, &mut d.nanos);
        sub(&self.entered, &earlier.entered, &mut d.entered);
        d
    }

    /// Only what missed, as `name misses/calls`, in [`Query::ALL`]'s order —
    /// the line a keystroke table prints.
    pub fn misses_line(&self) -> String {
        Query::ALL
            .iter()
            .filter(|&&q| self.misses(q) > 0)
            .map(|&q| format!("{} {}/{}", q.name(), self.misses(q), self.misses(q) + self.hits(q)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Every phase that took time, largest first, as `name 0.123s`.
    pub fn phases_line(&self) -> String {
        let mut rows: Vec<(Phase, u64)> =
            Phase::ALL.iter().map(|&p| (p, self.nanos[p.index()])).filter(|&(_, n)| n > 0).collect();
        rows.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        rows.iter().map(|&(p, n)| format!("{} {:.3}s", p.name(), n as f64 / 1e9)).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nested phase's time is its own, not its parent's too: the parent's
    /// exclusive time is its wall time less the child's. Red with the
    /// nested time left unsubtracted.
    #[test]
    fn a_phase_is_timed_exclusive_of_the_phases_inside_it() {
        let stats = Stats::default();
        {
            let _outer = stats.phase(Phase::Loop);
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _inner = stats.missed(Query::Sorted);
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let s = stats.snapshot();
        assert!(s.seconds(Phase::Query(Query::Sorted)) >= 0.040);
        let outer = s.seconds(Phase::Loop);
        assert!(outer >= 0.020 && outer < s.seconds(Phase::Query(Query::Sorted)), "the loop's own time was {outer}");
        assert_eq!(s.misses(Query::Sorted), 1);
        assert_eq!(s.since(&s), Snapshot::default());
    }
}
