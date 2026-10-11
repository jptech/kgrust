//! The binaries' allocator, chosen and tuned in one place.
//!
//! `kg`, `kanagawa`, `kg-lsp` and `xtask` each declare
//!
//! ```text
//! #[global_allocator]
//! static ALLOCATOR: kg_alloc::Allocator = kg_alloc::Allocator;
//! ```
//!
//! and a process that stays up between bursts of work — the language
//! server — calls [`purge_eagerly`] once, before it serves. Libraries never
//! name an allocator; a binary picks one.

/// mimalloc, for every binary.
///
/// The program-wide loop allocates and frees per round — rewrites,
/// snapshots, type maps — and macOS's allocator keeps the freed large
/// blocks rather than reuse them: `kg lsp`'s footprint grew ~90 MB a
/// keystroke on `risc-v.k`, 1.9 GB after twenty, where mimalloc holds
/// 0.54 GB after thirty, and a keystroke takes 18% fewer instructions
/// (`cargo xtask bench --filter keystroke-phases`, 2026-10-10;
/// `docs/journal.md`, "The keystroke, by phase"). `xtask` declares the
/// same so that its gates and benches measure what `kg` runs. The
/// backend's C++ allocates with its own `malloc`; nothing crosses the two.
pub use mimalloc::MiMalloc as Allocator;

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

/// `mi_option_purge_delay` (`mimalloc.h`): how long, in milliseconds, a
/// freed page waits before it is returned to the system. mimalloc keeps a
/// retired option in the enum as `deprecated_*` so that the numbering never
/// moves; the test reads the default back through this number, so an
/// upgrade that moved it anyway goes red rather than tuning another option.
const PURGE_DELAY: i32 = 15;

unsafe extern "C" {
    #[cfg(test)]
    fn mi_option_get(option: i32) -> std::ffi::c_long;
    fn mi_option_set(option: i32, value: std::ffi::c_long);
}

/// Return freed pages to the system as soon as they are free, not a
/// second later.
///
/// mimalloc purges only while it allocates or frees, so a server that goes
/// idle after a check keeps whatever the delay had not yet returned: with
/// the default, the freed pages of the last check stay in the editor's
/// footprint until the next keystroke. Measured on 2026-10-10 with
/// `cargo xtask mem`'s editor case:
///
/// - `array.k`, twelve edits each run to the fixpoint: the footprint after
///   them falls from 380 to 199 MB (its live heap is 133 MiB), and the
///   peak from 490 to 385 MB. Instructions rise 1.1%.
/// - `risc-v.k`, five edits: the peak footprint falls from ~1187 to
///   ~1070 MB, for 1.3% more instructions.
///
/// Forcing a collection after each check (`mi_collect`) ends at the same
/// 199 MB but leaves the peak where it was. `kg check` would save 6% of its
/// peak for 1.6% more instructions, and keeps the default: a one-shot
/// process returns everything when it exits.
pub fn purge_eagerly() {
    // SAFETY: the call takes two integers and writes one entry of
    // mimalloc's option table, which is safe from any thread at any time;
    // the purge reads the entry each time it runs.
    unsafe { mi_option_set(PURGE_DELAY, 0) }
}

/// [`Allocator`], counting the bytes it hands out: for a measurement that
/// reads the live heap around one request, which a process-wide peak
/// (`/usr/bin/time -l`, `cargo xtask mem`) cannot attribute — what a
/// request held at its height above what was live before it, and what it
/// left behind (`crates/kg-lsp/tests/workspace_cost.rs`). A binary that
/// wants it declares it as its global allocator in place of [`Allocator`];
/// the counts are of requested bytes, so they are the live heap, not the
/// pages the allocator keeps.
#[derive(Debug)]
pub struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

impl Counting {
    /// The bytes live now.
    pub fn live() -> usize {
        LIVE.load(Relaxed)
    }

    /// The most bytes live at once since the last call, which starts the
    /// next span at what is live now.
    pub fn take_peak() -> usize {
        let live = Counting::live();
        PEAK.swap(live, Relaxed).max(live)
    }
}

// SAFETY: every call goes to `Allocator` unchanged; the counters read only
// the layout, and a failed allocation is not counted.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { Allocator.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { Allocator.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An allocation is counted live until it is freed, and the peak holds
    /// the height between two readings. Red with the free left uncounted,
    /// or with the peak not raised by an allocation.
    #[test]
    fn the_counting_allocator_counts_what_is_live_and_its_peak() {
        let layout = Layout::from_size_align(1 << 20, 8).unwrap();
        let before = Counting::live();
        Counting::take_peak();
        // SAFETY: a non-zero layout, freed below with the same one.
        let p = unsafe { Counting.alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(Counting::live(), before + (1 << 20));
        // SAFETY: `p` came from `Counting.alloc(layout)`.
        unsafe { Counting.dealloc(p, layout) };
        assert_eq!(Counting::live(), before);
        assert!(Counting::take_peak() >= before + (1 << 20));
        assert_eq!(Counting::take_peak(), before);
    }

    /// `PURGE_DELAY` names the purge delay: the option it reads holds
    /// mimalloc's default of 1000 ms, which no other option has, and
    /// [`purge_eagerly`] sets it to 0. Red with any other option's number,
    /// or with `purge_eagerly` writing a different one.
    #[test]
    fn the_purge_delay_is_the_option_purge_eagerly_sets() {
        // SAFETY: as in `purge_eagerly`.
        let get = || unsafe { mi_option_get(PURGE_DELAY) };
        let before = get();
        if std::env::var_os("MIMALLOC_PURGE_DELAY").is_none() {
            assert_eq!(before, 1000, "option {PURGE_DELAY} is not mimalloc's purge delay");
        }
        purge_eagerly();
        assert_eq!(get(), 0);
        // SAFETY: as in `purge_eagerly`.
        unsafe { mi_option_set(PURGE_DELAY, before) };
    }
}
